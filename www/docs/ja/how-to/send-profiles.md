---
title: Pyroscope プロファイルを sigiro に送信する
description: 検証済みの Pyroscope Rust SDK から pprof プロファイルを Pyroscope 互換の HTTP で sigiro に送り、プロファイルの行を SQL で確認します。
sidebar:
  order: 7
---

このガイドでは、アプリケーションの CPU プロファイルを sigiro に送る方法を説明します。検証済みのファーストパーティ製 Pyroscope Rust SDK と、その pprof バックエンドを使います。

これは、エージェントのセッションを OpenTelemetry で記録する [エージェント自身のテレメトリの送信](/docs/how-to/agent-telemetry) とは別のものです。調べたいプロセスが pprof プロファイルを出力する場合に、このガイドを使ってください。

## 始める前に

sigiro サーバーが必要です。`sigiro serve` で起動するか、[クイックスタート](/docs/tutorials/quickstart#1-実行する) の Docker コマンドを使ってください。

Pyroscope 互換の HTTP は、sigiro のメインポート `9999` と OTLP HTTP ポート `4318` で利用できます。インジェストのトラフィックを意図的に分けるのでなければ、メインポートを使ってください。sigiro はポート `4040` ではプロファイルを待ち受けません。

Open 認証モードのセルフホストサーバーでは鍵は不要です。ホスト型 sigiro を含む Better Auth OAuth/JWT モードのサーバーでは、`Authorization: Bearer YOUR_TENANT_KEY` が必要です。

## pyroscope-rs を設定する

sigiro は、ファーストパーティの `pyroscope` クレートを pprof バックエンドとともに検証しています。このリポジトリでは現在バージョン `2.0.4` を固定しています。

```toml
[dependencies]
pyroscope = { version = "=2.0.4", features = ["backend-pprof-rs"] }
```

セルフホストの sigiro では、次のターゲットを使い、ヘッダーは付けません。

```rust
let endpoint = "http://localhost:9999";
let http_headers = None;
```

ホスト型 sigiro では、ポートなしの HTTPS オリジンを使い、OAuth アクセストークンを追加します。

```rust
use std::collections::HashMap;

let endpoint = "https://sigiro.com";
let mut headers = HashMap::new();
headers.insert(
    "Authorization".to_string(),
    format!("Bearer {}", std::env::var("SIGIRO_ACCESS_TOKEN")?),
);
let http_headers = Some(headers);
```

アプリケーションの起動時に、そのターゲットと任意のヘッダーマップを SDK に渡します。

```rust
use pyroscope::backend::pprof::PprofConfig;
use pyroscope::backend::{BackendConfig, pprof_backend};
use pyroscope::pyroscope::PyroscopeAgentBuilder;

let backend = pprof_backend(PprofConfig::default(), BackendConfig::default());
let mut builder = PyroscopeAgentBuilder::new(
    endpoint,
    "checkout",
    100,
    "pyroscope-rs",
    env!("CARGO_PKG_VERSION"),
    backend,
);
if let Some(headers) = http_headers {
    builder = builder.http_headers(headers);
}
let _profile_agent = builder.build()?.start()?;
```

`_profile_agent` はプロセスが動いている間ずっとスコープ内に保持してください。SDK は 10 秒ごとにバッチを送信します。ハンドルが早く破棄されると、それ以降のバッチは sigiro に届きません。

OAuth アクセストークンをソース管理に入れないでください。プロセスを起動する環境で `SIGIRO_ACCESS_TOKEN` を設定してください。

## HTTP の仕様

SDK は、次のいずれの同等なパスにも送信できます。

- `/`
- `/push.v1.PusherService/Push`
- `/ingest`
- `/ingest/pyroscope`

Rust SDK は上記のターゲットに `/push.v1.PusherService/Push` を付加します。そのため、ターゲットにすでにこのパスが含まれていると、sigiro 側で明確なエラーが出ないまま誤った URL に送信されることがあります。この SDK にはオリジンだけを渡してください。

sigiro は次のリクエスト形式を受け付けます。

- gzip 圧縮した Pyroscope `PushRequest` の場合は、`application/proto`、`binary/octet-stream`、またはコンテンツタイプなし
- 生の pprof バイトまたは `PushRequest` の場合は、名前がちょうど `profile` のファイルフィールドを持つ `multipart/form-data`

クエリパラメーター `name` は、sigiro がボディをデコードする方法を変えます。`name` があれば、sigiro は `profile` のバイトを生の pprof として扱い、最初の `{` より前のテキストをサービス名として使います。`name` がなければ、ボディは `PushRequest` としてデコードできる必要があり、各シリーズには空でない `service_name` ラベルが必要です。`name` なしで生の pprof を送ったり、`service_name` のない `PushRequest` を送ったりすると、`400` が返り、何も保存されません。

sigiro は gzip を受け付け、既定では各リクエストを展開後 8 MiB に制限します。HTTP ボディの上限を超えるリクエストには `413` が返ります。不正な形式またはサポートされていないプロファイルには `400`、Better Auth OAuth/JWT のベアラートークンがないか無効な場合は `401`、容量の制限に達した場合は `507` が返ります。`200` は sigiro がバッチを受け付けたことを意味します。

送信するのは pprof データだけにしてください。sigiro は Java Flight Recorder のデータをデコードしないため、このガイドは Java や JFR との互換性をうたいません。他の言語の SDK の設定は、エンドツーエンドで実行可能な検証ができるまで省略しています。

## 動作を確認する

アプリケーションを起動し、少なくとも 10 秒間処理をさせます。その後、メインの HTTP API からプロファイルの行を読み返します。

```bash
curl -s -X POST http://localhost:9999/v1/query \
  --data "SELECT service_name, profile_type, unit, count(*) AS samples
          FROM sigiro_profiles
          WHERE service_name = 'checkout'
            AND timestamp > now() - INTERVAL '1 hour'
          GROUP BY 1, 2, 3
          ORDER BY samples DESC"
```

ホスト型 sigiro では、オリジンを変更し、`-H "Authorization: Bearer YOUR_TENANT_KEY"` を追加します。結果が空でなければ、SDK の送信、プロファイルのデコード、HTTP での読み返しが確認できたことになります。SDK の 1 間隔が過ぎても結果が空の場合、たいていはプロファイルのハンドルが破棄された、ターゲットに余分なパスが含まれている、またはクエリのサービス名が SDK の設定と一致していない、のいずれかです。

## 次のステップ

- [ホスト型 sigiro へのテレメトリ送信](/docs/how-to/hosted-onboarding) — ホスト型の認証と、その他のテレメトリシグナルを設定します
- [テーブルについて](/docs/explanation/tables) — 各テーブルにどのテレメトリが保存されているかを確認します
