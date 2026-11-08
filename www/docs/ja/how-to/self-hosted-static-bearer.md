---
title: セルフホスト環境で静的ベアラートークンを使う
description: クエリ API と OTLP HTTP・gRPC に共通のシークレットを設定します。
sidebar:
  order: 8
---

ローカルのセルフホストサーバーで `SIGIRO_STATIC_BEARER_TOKEN` を設定すると、クエリ HTTP、OTLP HTTP、OTLP gRPC の各リスナーに同じトークンが必要になります。`openssl rand -hex 32` でランダムな値を生成し、シークレット管理システムからサーバーに渡してください。32 バイト以上の ASCII bearer トークン文字が必要です。空、短すぎる値、不正な文字、UTF-8 以外の値では起動に失敗します。設定しなければローカルサーバーは Open モードのままであり、リクエストを認証しません。Open モードのリスナーを信頼できないネットワークに公開しないでください。

CLI や stdio MCP クライアントには同じ値を `SIGIRO_ACCESS_TOKEN` または `--token` で渡します。OTLP エクスポーターと HTTP クライアントは `Authorization` ヘッダーに `Bearer ` とトークンを指定します。このモードでは大文字小文字を含めて `Bearer` スキームのみを受け付け、`AgentAuth` は受け付けません。認証されたリクエストはすべてローカルの `default` テナントに属します。`/health` は公開されたままです。`/admin/*` には別の `SIGIRO_ADMIN_TOKEN` が必要で、静的トークンは管理者トークンではありません。

このモードは**ローカルストレージのセルフホスト環境専用**です。`SIGIRO_AUTH_ISSUER`、`SIGIRO_AUTH_AUDIENCE`、`SIGIRO_AUTH_JWKS_URL`、分散 PostgreSQL カタログ、`SIGIRO_QUERY_SURFACE=quack` と併用すると起動に失敗します。メインの HTTP クエリリスナーではネイティブ TLS は有効にならないため、リモートアクセスには TLS 終端リバースプロキシを使用してください。OTLP HTTP・gRPC のネイティブ TLS には `SIGIRO_TLS_CERT` と `SIGIRO_TLS_KEY` を設定するか、信頼できるプロキシで TLS を終端します。平文の接続でトークンを送らないでください。ローテーションにはクライアントの更新とサーバー再起動が必要です。トークンを同時に 2 個受け入れる移行期間も、取り込みとクエリの権限分離もありません。
