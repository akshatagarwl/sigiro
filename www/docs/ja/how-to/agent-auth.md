---
title: Agent Auth でエージェントをオンボーディングする
description: Sigiro の Agent Auth API でエージェントホストを登録し、最小限の capability を付与して、資格情報をローテーションまたは失効させます。
sidebar:
  order: 2
---

人ではなくソフトウェアがホステッド Sigiro に認証する場合は Agent Auth を使用します。
Sigiro は委任 ID をサポートします。ユーザーがエージェントを一度承認してワークスペースに
関連付けると、エージェントは自身の鍵と短期トークンを使って無人で動作できます。

別の Sigiro 実行ファイルは不要です。公式の `@auth/agent` SDK をエージェントホストに
組み込んでください。Sigiro は自律型エージェント ID をサポートしません。

## 始める前に

最初にホステッド Sigiro アカウントを作成するか、サインインします。

```bash
sigiro signup --name "Your Name" --email you@example.com
# 既存のアカウント：
sigiro auth login
```

アカウントは Sigiro ワークスペース 1 つだけに所属している必要があります。Agent Auth は
承認されたエージェントをそのワークスペースに関連付けます。

SDK には永続的な秘密ストレージを使用してください。ホストとエージェントの Ed25519
秘密鍵、および接続レコードが保存されます。既定のメモリストレージは短時間の例専用です。
失うと、新しい ID の登録が必要です。

## 1. Sigiro を検出する

ライフサイクルルートをハードコードせず、稼働中のプロバイダードキュメントを確認します。

```bash
curl -fsS https://sigiro.com/.well-known/agent-configuration | jq
```

issuer が `https://sigiro.com/api/auth`、保護対象リソースが
`https://sigiro.com`、`modes` が `delegated` のみであることを確認します。

利用可能な capability を表示します。

```bash
curl -fsS https://sigiro.com/api/auth/capability/list | jq
```

プロセスに必要なものだけを要求してください。OTLP エクスポーターに必要なのは
`telemetry:ingest` だけで、読み取りやアラート管理の capability は不要です。

## 2. 公式 SDK を追加する

```bash
aube add @auth/agent@0.6.2
```

Sigiro のサービスオリジンでクライアントを作成します。本番環境では、シークレットストアを
バックエンドにする `KVStorage` など、永続的な `storage` を渡してください。秘密鍵を
ソース管理、ログ、スクリーンショット、暗号化されていないアプリケーション状態に保存しないでください。

```ts
import { AgentAuthClient, MemoryStorage } from '@auth/agent';

const client = new AgentAuthClient({
  urls: ['https://sigiro.com'],
  allowDirectDiscovery: true,
  hostName: 'production-telemetry-host',
  storage: new MemoryStorage(), // 本番環境では永続的な秘密ストレージに置き換えます。
  onApprovalRequired: (approval) => {
    const url = approval.verification_uri_complete ?? approval.verification_uri;
    approvalUi.show({ url, code: approval.user_code });
  },
});

const providers = await client.init();
if (providers.length !== 1) throw new Error('Sigiro の検出に失敗しました');
```

`approvalUi.show` は、認証済みで内容を永続化しないオペレーター UI を表します。承認 URL
やコードをアプリケーションログに書き込んではなりません。

`connectAgent` はホストとエージェントの Ed25519 鍵を生成します。Sigiro の検出文書と
本番設定では動的ホスト登録が許可されているため、この手順では別のホスト登録コマンドや
トークンは不要です。管理者が事前に準備したホストでは、1 回限りの登録トークンを使う SDK の
`enrollHost` フローを使用します。2 つの手順を混在させないでください。

## 3. エージェントを登録して承認する

委任モードで接続し、テレメトリー取り込みだけを要求します。

```ts
const connection = await client.connectAgent({
  provider: 'https://sigiro.com',
  mode: 'delegated',
  name: 'production telemetry exporter',
  capabilities: ['telemetry:ingest'],
  reason: "Send this service's OpenTelemetry data to Sigiro",
  preferredMethod: 'device_authorization',
});

if (connection.status !== 'active') {
  throw new Error(`エージェントが有効ではありません: ${connection.status}`);
}
```

表示された検証 URL を開き、Sigiro にサインインしてコードを確認し、要求された
capability を承認します。SDK は承認成功または期限切れまでポーリングします。承認 URL と
コードは一時的ですが、ログやサポートチケットには記録しないでください。

承認後は `connection.agentId` を SDK の永続状態と一緒に保持します。秘密鍵を
エクスポートしたりコピーしたりしないでください。

## 4. テレメトリーを認証する

テレメトリー要求の直前に、capability を限定した新しいトークンを署名します。

```ts
const authorization = await client.signJwt({
  agentId: connection.agentId,
  audience: 'https://sigiro.com',
  capabilities: ['telemetry:ingest'],
});

const headers = {
  Authorization: `Bearer ${authorization.token}`,
};
```

このヘッダーを `https://sigiro.com/v1/traces`、`/v1/metrics`、
`/v1/logs`、または `/v1development/profiles` への OTLP/HTTP 要求に付けます。
エージェントトークンは短命です。エクスポーターのトランスポートは有効期限前に新しい
トークンを取得し、1 つのトークンを長期環境変数に保存しないでください。

Sigiro はテレメトリーを受け入れる前に、エージェント状態、ワークスペースとの関連付け、
audience、リプレイ状態、`telemetry:ingest` 付与を検証します。署名済みトークンを別の
要求に再利用しないでください。

## 5. エージェント鍵をローテーションする

SDK を通じて鍵をローテーションします。

```ts
await client.rotateAgentKey(connection.agentId);
```

登録済み公開鍵が置き換わり、設定済みストレージの秘密鍵が更新されます。ローテーション後は
以前の鍵で署名したトークンを破棄してください。ホストが引き続きエージェントを管理できるよう、
再起動後も SDK ストレージを利用可能にします。

## 6. 失効して確認する

否定テスト用のトークンだけを作成し、エージェントを失効させます。

```ts
const tokenBeforeRevocation = await client.signJwt({
  agentId: connection.agentId,
  audience: 'https://sigiro.com',
  capabilities: ['telemetry:ingest'],
});

await client.disconnectAgent(connection.agentId);
```

`disconnectAgent` はリモート ID と capability 付与を失効させてから、ローカル接続を
削除します。以後、`tokenBeforeRevocation` を使った要求は失敗する必要があります。Sigiro の
OTLP/HTTP エンドポイントは、失効したエージェントトークンに HTTP `401` を返します。
受け入れられた場合は失効失敗として扱い、エクスポーターを停止してください。

単一のエージェントではなくホスト全体を廃止する場合は、承認した Sigiro アカウントから
ホストを失効させます。ホストの失効により、そのホストに属する全エージェントも失効します。

## トラブルシューティング

- **検出に失敗する:** プロバイダー URL には issuer パスではなく
  `https://sigiro.com` を使用し、well-known ドキュメントを `curl` で確認します。
- **承認が期限切れになる:** `connectAgent` を再実行し、新しい要求の期限内に承認します。
- **capability がない:** `telemetry:ingest` を明示的に要求します。無関係な capability を
  追加して回避しないでください。
- **再起動後に ID が消える:** `MemoryStorage` を永続的な `KVStorage` バックエンドに
  置き換え、そのバックエンドを秘密情報として保護します。
- **署名トークンがリプレイとして拒否される:** 要求ごとに新しいトークンを署名します。
  Sigiro の認証インスタンス間でリプレイ保護は共有されています。
- **失効したエージェントがデータを送信できる:** プロセスを停止し、失敗応答を保存して
  Sigiro に連絡してください。原因が分かるまで代替 ID を作成しないでください。

人のサインアップ、OAuth ログイン、通常のアクセストークンについては
[ホステッド Sigiro にテレメトリーを送る](/docs/how-to/hosted-onboarding)を参照してください。
Sigiro CLI の動作については [CLI リファレンス](/docs/reference/cli)を参照してください。
