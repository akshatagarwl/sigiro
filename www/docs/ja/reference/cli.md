---
title: CLI
description: >-
  sigiro のすべてのサブコマンド、その引数、および読み取る環境変数。sigiro には 7
  つのサブコマンドがあり、クライアント側はインデントされた JSON を出力します。
sidebar:
  order: 2
---

`sigiro` には 7 つのサブコマンドがあります。すべてのサブコマンドは、成功時にインデントされた
JSON を出力します。2 つ目の出力形式は存在せず、TTY 検出も行いません。

新規ユーザーは `sigiro signup --name "Your Name" --email you@example.com` を実行します。
6 桁の OTP がメールで届き、ターミナルで安全に入力すると、個人用ワークスペースを 1 つだけ
作成・有効化し、ブラウザーなしで OAuth デバイス認証を内部的に完了して、更新・アクセス資格情報を
OS の資格情報ストアに保存します。既存アカウントはブラウザーベースの `sigiro auth login` を使用します。
`sigiro auth status`、`sigiro auth token`、`sigiro auth logout` でログイン状態を管理します。

グローバルフラグ `--token <TOKEN>` は、自動化用のベアラートークンを明示的に設定します。
このフラグは `SIGIRO_ACCESS_TOKEN` を上書きします。Open モードのセルフホスト型サーバーには不要ですが、静的ベアラー認証を設定したサーバーには必要です。

すべてのクライアントサブコマンドは、`SIGIRO_ENDPOINT` に指定されたサーバーに接続します。
デフォルトは `http://localhost:9999` です。

## ホスト型エージェント認証

Sigiro は公式 Agent Auth 実装を内部利用し、別のオンボーディング CLI は公開しません。
委任ホスト登録、1 回限りの承認、最小権限の capability 付与、トークン利用、鍵の
ローテーション、失効については、
[Agent Auth でエージェントをオンボーディングする](/docs/how-to/agent-auth)を参照してください。

## serve

OTLP gRPC サーバー、OTLP HTTP サーバー、およびクエリ API を起動します。

| フラグ                    | 上書き対象              | デフォルト              |
| ------------------------- | ----------------------- | ----------------------- |
| `--data-dir <PATH>`       | `SIGIRO_DATA_DIR`       | `~/.local/share/sigiro` |
| `--http-port <PORT>`      | `SIGIRO_HTTP_PORT`      | `9999`                  |
| `--otlp-http-port <PORT>` | `SIGIRO_OTLP_HTTP_PORT` | `4318`                  |
| `--otlp-grpc-port <PORT>` | `SIGIRO_GRPC_PORT`      | `4317`                  |

初回起動時には、クエリ拡張機能がデータディレクトリにダウンロードされます。このダウンロードには
30〜90 秒かかり、その間は起動がブロックされます。

## query

`sigiro query "<SQL>"` は、ローカルクライアントではなく**フェデレーテッドコーディネーター**です。
1 つ以上のエッジを解決し、各エッジに部分クエリをプッシュして、その部分結果を 1 つの回答に
まとめます。

| フラグ                    | 型               | デフォルト |
| ------------------------- | ---------------- | ---------- |
| `<SQL>`                   | 文字列           | なし       |
| `--edge <NAME=HOST:PORT>` | 繰り返し指定可能 | なし       |
| `--full-scan`             | フラグ           | オフ       |

エッジの解決順序は次のとおりです。`SIGIRO_EDGES`（カンマ区切りの
`name=host:port`）と `--edge` はそのまま使用され、いずれか一方が設定されていればディスカバリーは
スキップされます。どちらも設定されていない場合、sigiro は `tailscale status --json` を実行し、
`tag:sigiro-edge` のタグが付いたすべてのピアを取得します。解決されたエッジが 0 個の場合、
コマンドは何も対象がない状態で実行するのではなく、失敗します。

したがって `sigiro query` は、単純なローカルサーバーに対しては**動作しません**。エッジは
`SIGIRO_QUERY_SURFACE=quack` でデータを提供している必要があり、`--edge` で指定するか、
tailnet 上で見つける必要があります。1 つのローカルサーバーに対して SQL を実行するには、
代わりに SQL を `/v1/query` に POST してください。

`--full-scan` は、`timestamp` に対する `WHERE` 境界のないクエリを許可します。sigiro は
デフォルトではそのようなクエリを拒否します。

## status

`sigiro status` はサーバーの `/health` エンドポイントを読み取り、ヘルスブロックを出力します。
`/health` にキーは不要です。

## healthcheck

`sigiro healthcheck` はローカルの `/health` エンドポイントをプローブし、サーバーが準備完了の場合に
のみ `0` を返して終了します。distroless イメージにはシェルも `curl` も含まれていないため、これが
コンテナの `HEALTHCHECK` になります。

## mcp

`sigiro mcp` は stdio Model Context Protocol サーバーを起動します。
`list_services` と `run_query` を公開し、
分析エージェントは HTTP リクエストを直接組み立てずに Sigiro を調査できます。
