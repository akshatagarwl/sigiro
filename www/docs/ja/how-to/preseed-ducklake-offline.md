---
title: オフライン環境向けに DuckLake を事前配置する
description: プロキシ下またはオフラインのセルフホスト環境で、DuckDB のバージョンとプラットフォームに合う拡張を起動前に配置します。
sidebar:
  order: 8
---

Sigiro のリリースイメージに **DuckLake は同梱されていません**。新規ローカルデータディレクトリの初期化時、Sigiro は DuckDB に `INSTALL ducklake; LOAD ducklake;` を実行させます。ダウンロードと HTTP リダイレクトの処理は DuckDB 側の責任です。プロキシが HTTP 302 を返すと、`curl -L` では取得できても DuckDB のインストールは失敗する場合があります。Sigiro 独自のリダイレクト設定やオフラインダウンローダーはありません。接続可能なホストでキャッシュを作成し、実行環境にマウントしてください。

この手順は**ローカルファイルカタログ**用です。PostgreSQL カタログにはさらに DuckDB の `postgres` 拡張、S3 データファイルには `httpfs` 拡張が必要です。分散構成では同じバージョンとプラットフォームの追加拡張も配置してください。この単一ファイルの手順だけで分散構成のオフライン動作を保証することはできません。

## 1. 正しい成果物を選ぶ

現在の Sigiro ソースが固定する DuckDB は **`v2.0.0-alpha41489`** です（Rust の `duckdb` クレートのバージョンではありません）。DuckDB の配置先は `<extension_directory>/<DuckDB version>/<platform>/`。リリースイメージでは x86-64 に `linux_amd64`、ARM64 に `linux_arm64` を選びます。これらは DuckDB のプラットフォーム名であり Rust のターゲット名とは異なります。Apple Silicon のネイティブビルドは `osx_arm64` が必要です。macOS 用拡張を Linux コンテナにコピーしないでください。Sigiro 更新時には `src/storage.rs` の固定バージョンを再確認し、バージョンまたはプラットフォームが変われば成果物を取得し直します。

**ネットワーク接続のある Linux ステージングホスト**で、対象プラットフォームと、その URL に対して独立に承認された圧縮ファイルの SHA-256 値を指定して実行します。

```bash
set -eu
DUCKDB_VERSION=v2.0.0-alpha41489
PLATFORM=linux_amd64 # ARM64 リリースイメージでは linux_arm64
: "${DUCKLAKE_GZ_SHA256:?Set the approved SHA-256 digest for this exact URL}"
STAGE="${PWD}/sigiro-extension-cache"
DEST="${STAGE}/${DUCKDB_VERSION}/${PLATFORM}"
mkdir -p "$DEST"
curl --fail --location --proto '=https' --proto-redir '=https' \
  "https://extensions.duckdb.org/${DUCKDB_VERSION}/${PLATFORM}/ducklake.duckdb_extension.gz" \
  --output "$DEST/ducklake.duckdb_extension.gz"
printf '%s  %s\n' "$DUCKLAKE_GZ_SHA256" "$DEST/ducklake.duckdb_extension.gz" | sha256sum -c -
gzip -dc "$DEST/ducklake.duckdb_extension.gz" > "$DEST/ducklake.duckdb_extension"
rm "$DEST/ducklake.duckdb_extension.gz"
test -s "$DEST/ducklake.duckdb_extension"
```

URL は [DuckDB の直接ダウンロード形式](https://duckdb.org/docs/current/extensions/advanced_installation_methods.html) に従います。`.gz` は**展開が必要**です。期待するハッシュ値は信頼できる成果物承認手続きから入手してください。取得した未検証ファイルをハッシュ化し、それを期待値とするのは検証ではありません。HTTPS と SHA-256 は DuckDB の拡張署名検証の代わりにはなりません。`SIGIRO_ALLOW_UNSIGNED_EXTENSIONS` は未設定のままにしてください（既定値は false）。`.info` メタデータファイルを自作しないでください。署名またはバージョン検証に失敗した場合、署名検証を無効にせず、対応する公式成果物を取得してください。

## 2. キャッシュをマウントして初回起動する

`sigiro-extension-cache` ディレクトリを丸ごとオフラインホストに転送します。distroless リリースイメージでは、例えば `/var/lib/sigiro/extensions` に**ディレクトリ**をマウントし、`SIGIRO_EXTENSION_DIR=/var/lib/sigiro/extensions` を設定します。DuckLake カタログ、Parquet、テンポラリファイルのため、別途書き込み可能な `SIGIRO_DATA_DIR`（イメージ内の既定値は `/var/lib/sigiro`）を永続化します。コンテナは **nonroot** ユーザーで動くため、両方のマウントをそのユーザーが通過・書き込みできるようにします。ファイルを配置済みでも拡張のルートは書き込み可能でなければなりません。Sigiro はディレクトリを作成し、DuckDB はキャッシュに書き込む場合があります。読み取り専用マウントや、事前配置したディレクトリを隠す `/var/lib/sigiro` への別マウントは使えません。distroless イメージ内のシェルや `curl` には依存しないでください。

ネイティブバイナリの場合の起動方法:

```bash
SIGIRO_DATA_DIR=/path/to/writable/data \
SIGIRO_EXTENSION_DIR=/path/to/writable/sigiro-extension-cache \
sigiro serve
```

ローカルモードで `SIGIRO_EXTENSION_DIR` が未設定なら `<SIGIRO_DATA_DIR>/extensions` が使われます。変数には `v2.0.0-alpha41489/linux_amd64` ではなく、その**親のキャッシュルート**を指定します。再起動後もキャッシュを保持してください。コンテナを再ビルドしても DuckLake はイメージに追加されません。

## 3. リスナーだけでなくストレージも確認する

起動ログに `loaded DuckDB extension: ducklake` と `default project initialized` があることを確認します。ポート `4318` に OTLP サンプルを送ってからクエリを実行します（カウントが増えるまで取り込みのフラッシュ間隔を待つ必要があります）:

```bash
SINCE="$(date -u +'%Y-%m-%d %H:%M:%S')"
START_NS="$(date +%s)000000000"
END_NS="$(( $(date +%s) + 1 ))000000000"
curl -fsS -X POST http://127.0.0.1:4318/v1/traces \
  -H 'Content-Type: application/json' \
  --data "{\"resourceSpans\":[{\"resource\":{\"attributes\":[{\"key\":\"service.name\",\"value\":{\"stringValue\":\"offline-probe\"}}]},\"scopeSpans\":[{\"spans\":[{\"traceId\":\"33333333333333333333333333333333\",\"spanId\":\"4444444444444444\",\"name\":\"offline-probe\",\"kind\":1,\"startTimeUnixNano\":\"${START_NS}\",\"endTimeUnixNano\":\"${END_NS}\"}]}]}]}"
curl -fsS -X POST http://127.0.0.1:9999/v1/query \
  -H 'Content-Type: text/plain' \
  --data "SELECT count(*) AS n FROM lake.sigiro_spans WHERE timestamp >= TIMESTAMP '${SINCE}' AND service_name = 'offline-probe'"
```

現在の main では `/health` の `ready` **だけでストレージと取り込みの動作は証明できません**。起動時の失敗を readiness より前に検出する修正は [#359](https://github.com/akshatagarwl/sigiro/pull/359) でレビュー中です。使用中のバイナリに含まれると仮定しないでください。ログと両方の動作経路を確認します。拡張の欠落、バージョン・プラットフォーム違い、署名エラー、書き込み不可のキャッシュはデプロイ失敗です。成果物またはマウントを修正して再起動してください。このガイドは運用上の回避策であり、イメージへの同梱や DuckDB のプロキシリダイレクト修正ではありません。配置規則は [DuckDB の拡張インストール先](https://duckdb.org/docs/current/extensions/installing_extensions.html) を参照してください。
