---
title: CLI
description: >-
  sigiro のすべてのサブコマンド、その引数、および読み取る環境変数について説明します。sigiro には 7
  つのサブコマンドがあります。それぞれがインデントされた JSON を出力します。
sidebar:
  order: 2
---
`sigiro` には 7 つのサブコマンドがあります。すべてのサブコマンドは、成功時にインデントされた
JSON を出力します。2 つ目の出力形式は存在せず、TTY 検出も行いません。

グローバルフラグ `--key <TOKEN>` は、ホスト型エンドポイント用のベアラートークンを設定します。
このフラグは `SIGIRO_API_KEY` を上書きします。セルフホスト型サーバーはトークンを無視します。

すべてのクライアントサブコマンドは、`SIGIRO_ENDPOINT` に指定されたサーバーに接続します。
デフォルトは `http://localhost:9999` です。

## serve

OTLP gRPC サーバー、OTLP HTTP サーバー、およびクエリ API を起動します。

| フラグ | 上書き対象 | デフォルト |
| --- | --- | --- |
| `--data-dir <PATH>` | `SIGIRO_DATA_DIR` | `~/.local/share/sigiro` |
| `--http-port <PORT>` | `SIGIRO_HTTP_PORT` | `9999` |
| `--otlp-http-port <PORT>` | `SIGIRO_OTLP_HTTP_PORT` | `4318` |
| `--otlp-grpc-port <PORT>` | `SIGIRO_GRPC_PORT` | `4317` |

初回起動時には、クエリ拡張機能がデータディレクトリにダウンロードされます。このダウンロードには
30〜90 秒かかり、その間は起動がブロックされます。

## diagnose

`sigiro diagnose <SERVICE>` は、1 つのサービスに対して 1 つの診断ブロックを返します。

| フラグ | 型 | デフォルト |
| --- | --- | --- |
| `--from <EPOCH_SECONDS>` | 整数 | 現在から 15 分前 |
| `--to <EPOCH_SECONDS>` | 整数 | 現在 |

`--from` と `--to` はエポック**秒**です。対応する HTTP フィールド名は
`from_ts` と `to_ts` であり、こちらはエポック**マイクロ秒**です。どちらの HTTP フィールドも
省略可能です。両方を省略した場合、サーバーは直近 15 分間を読み取ります。これは CLI が使用するのと
同じウィンドウです。

このコマンドは JSON を出力します。出力を絞り込むには、`jq` にパイプしてください。CLI 自体は
フィルターを一切適用しないため、CLI と API は同じドキュメントを返します。

## anomalies

`sigiro anomalies` は、検出されたレジームシフトを、変化の大きいものから順に一覧表示します。
各行には、シグナル横断的な `incident_id` が含まれます。

| フラグ | 型 | デフォルト |
| --- | --- | --- |
| `--service <NAME>` | 文字列 | すべてのサービス |
| `--from <EPOCH_SECONDS>` | 整数 | 下限なし |
| `--to <EPOCH_SECONDS>` | 整数 | 上限なし |

各行は、スケジュールされたパスが書き込む `sigiro_anomalies` テーブルから取得されます。
リクエスト時に計算される項目はありません。

## query

`sigiro query "<SQL>"` は、ローカルクライアントではなく**フェデレーテッドコーディネーター**です。
1 つ以上のエッジを解決し、各エッジに部分クエリをプッシュして、その部分結果を 1 つの回答に
まとめます。

| フラグ | 型 | デフォルト |
| --- | --- | --- |
| `<SQL>` | 文字列 | なし |
| `--edge <NAME=HOST:PORT>` | 繰り返し指定可能 | なし |
| `--full-scan` | フラグ | オフ |

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

## fetch-extensions

`sigiro fetch-extensions` は、最初の `sigiro serve` の前に、クエリ拡張機能をデータディレクトリに
ダウンロードします。sigiro のインストール後に一度実行するか、外部ネットワークにアクセスできない
ホストへのデプロイ前に実行してください。

## healthcheck

`sigiro healthcheck` はローカルの `/health` エンドポイントをプローブし、サーバーが準備完了の場合に
のみ `0` を返して終了します。distroless イメージにはシェルも `curl` も含まれていないため、これが
コンテナの `HEALTHCHECK` になります。
