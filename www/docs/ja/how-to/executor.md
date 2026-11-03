---
title: sigiro を Executor に接続する
description: sigiro の OpenAPI API またはドキュメント MCP サーバーを Executor に追加し、得られたツールをエージェントに公開します。
sidebar:
  order: 6
---

[Executor](https://executor.sh/) はエージェントのための統合レイヤーです。OpenAPI 仕様、MCP サーバー、GraphQL エンドポイントを 1 つのツールカタログに取り込みます。Executor のプラグインを書かずに sigiro を追加できます。

Executor はカタログを埋めるために [integrations.sh](https://integrations.sh/) レジストリも読み取り、sigiro はそこに自身のインターフェースを公開しているため、何かを貼り付ける前に Executor 内の検索で sigiro が見つかることがあります。以下の手順は明示的な取り込みで、どちらの場合でも機能します。

## sigiro API を追加する

公開されている OpenAPI ドキュメントは次のとおりです。

```
https://sigiro.com/openapi.json
```

Executor の Web UI で次の操作を行います。

1. **Add Integration** を開きます。
2. OpenAPI の URL を貼り付けます。
3. Executor に操作を検出させ、インデックスを作成させます。
4. そのインテグレーションの接続を作成します。
5. API が求める場合は、その接続にホスト型 sigiro の OAuth アクセストークンを追加します。
6. ツールごとにポリシーを設定します。SQL を実行するツールやテレメトリを調べるツールには承認を使ってください。

仕様は公開されており、読むのに鍵は不要です。ホスト型 API 自体には引き続き OAuth アクセストークンが必要です。セルフホストの sigiro でも、ご自身のインスタンスが提供する OpenAPI の URL を使って同じ方法で取り込めます。セルフホストのサーバーはオープンモードで動作し、どのリクエストも認証しないため、鍵はまったく不要です。

Executor には CLI もあります。同等の取り込みは次の形になります。

```bash
executor call executor openapi addIntegration '{
  "spec": "https://sigiro.com/openapi.json",
  "namespace": "sigiro",
  "baseUrl": "https://sigiro.com"
}'
```

仕様に記載されたサーバー URL が、Executor に呼び出させたい URL と異なる場合は、`baseUrl` フィールドを使ってください。認証は、公開仕様の URL ではなく、作成された接続に設定してください。

有効になったことを確認します。

```bash
executor tools integrations
```

## sigiro ドキュメントの MCP サーバーを追加する

ドキュメントサイトは、公開の読み取り専用 MCP サーバーも提供しています。

```
https://sigiro.com/mcp/docs
```

Executor で MCP インテグレーションとして追加し、streamable HTTP トランスポートを選択します。これにより、エージェントは検索やページ取得などのドキュメントツールを使えるようになります。ワークスペースのテレメトリにはアクセスできません。`/docs` を除いた同じ URL `https://sigiro.com/mcp` はテレメトリツールを追加し、OAuth サインインが必要です。

ライブのテレメトリ操作には OpenAPI のインテグレーションを使います。エージェントが sigiro のドキュメントを探したり読んだりする必要があるときは MCP のインテグレーションを使います。両方を同じ Executor ワークスペースに追加できます。

## エージェントを接続する

Executor がインテグレーションのインデックスを作成したら、エージェントを MCP 経由で Executor に接続します。エージェントが使うのは Executor の MCP エンドポイントであり、sigiro 専用のプラグインは不要です。デプロイで使うエンドポイントとトランスポートについては、[Executor の MCP 接続ガイド](https://executor.sh/docs) に従ってください。

## 取り込みを確認する

エージェントに使わせる前に、Executor でインテグレーションを確認してください。sigiro API の取り込みでは、サービス、トレース、アラート、クエリの操作が公開されているはずです。ドキュメントの取り込みでは、ドキュメント検索とページ取得が公開されているはずです。

次に、まず読み取り専用の呼び出しを 1 つ試します。ホスト型 API の接続では、リクエストに OAuth アクセストークンが含まれていること、そして返ってきたデータが意図した sigiro テナントのものであることを確認してください。OAuth アクセストークンをこのページ、OpenAPI の URL、リポジトリにコミットされるエージェントのプロンプトに含めないでください。

## 次のステップ

- [API リファレンス](/docs/reference) — 生成された操作の詳細を確認します
- [ホスト型 sigiro へのテレメトリ送信](/docs/how-to/hosted-onboarding) — ホスト型のエンドポイントとテナント接続を作成します
- [クイックスタート](/docs/tutorials/quickstart) — sigiro をローカルで実行し、テストデータを送信します
