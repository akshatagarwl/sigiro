---
title: プロファイルを sigiro に送信する
description: Go 向けの otelprofile、OpenTelemetry eBPF プロファイラー、または OpenTelemetry Collector から OTLP で継続的プロファイルを sigiro に送り、プロファイルの行を SQL で確認します。
sidebar:
  order: 7
---

このガイドでは、継続的プロファイル（CPU、ヒープ、その他の pprof プロファイル種別）を sigiro に送る方法を説明します。sigiro が受け付けるプロファイルの形式は 1 つだけです。OpenTelemetry のプロファイルシグナルである、HTTP または gRPC 上の OTLP プロファイルです。

これは、エージェントのセッションを OpenTelemetry で記録する [エージェント自身のテレメトリの送信](/docs/how-to/agent-telemetry) とは別のものです。プロセスが CPU 時間とメモリをどこで使っているかを見たいときに、このガイドを使ってください。

## 始める前に

sigiro サーバーが必要です。`sigiro serve` で起動するか、[クイックスタート](/docs/tutorials/quickstart#1-実行する) の Docker コマンドを使ってください。

セルフホストのサーバーは、OTLP/HTTP をポート `4318` とメインポート `9999` で、OTLP/gRPC をポート `4317` で受け付けます。ホスト型 sigiro は OTLP/HTTP のみを、ポートなしの `https://sigiro.com` で受け付けます。

Open 認証モードのセルフホストサーバーでは鍵は不要です。ホスト型 sigiro を含む Better Auth OAuth/JWT モードのサーバーでは、`Authorization: Bearer YOUR_TENANT_KEY` が必要です。

OpenTelemetry のプロファイルシグナルは、上流でまだ開発中で、ワイヤーフォーマットはリリース間で変わってきました。sigiro は `opentelemetry-proto` v1.10 のレイアウトを読みます。別のリビジョンで作られたプロデューサーが送るプロファイルは、sigiro が `400` で拒否することがあります。

## Go: otelprofile

[otelprofile](https://github.com/akshatagarwl/otelprofile-go) は、Go 自身のランタイムプロファイラーで Go プログラムを内側からプロファイルし、OTLP プロファイルとしてエクスポートします。モジュールに追加します。

```bash
go get github.com/akshatagarwl/otelprofile-go@v0.3.0
```

プログラムの起動時に一度だけ開始し、終了時に停止します。

```go
p, err := otelprofile.Start(otelprofile.Config{
    ServiceName: "checkout",
    Endpoint:    "http://localhost:4318",
})
if err != nil {
    log.Fatal(err)
}
defer p.Stop()
```

既定では、ヒーププロファイルを 15 秒ごとに、CPU プロファイルを 30 秒ごとに送ります。goroutine、mutex、block のプロファイルも送るには、`ProfileTypes`（または `OTEL_GO_X_PROFILES_TYPES=all`）を設定します。

ホスト型 sigiro では、エンドポイントを `https://sigiro.com` にして鍵を追加します。

```go
p, err := otelprofile.Start(otelprofile.Config{
    ServiceName: "checkout",
    Endpoint:    "https://sigiro.com",
    Headers: map[string]string{
        "Authorization": "Bearer " + os.Getenv("SIGIRO_ACCESS_TOKEN"),
    },
})
```

対応するフィールドを空にしておけば、標準の `OTEL_EXPORTER_OTLP_ENDPOINT`、`OTEL_EXPORTER_OTLP_HEADERS`、`OTEL_SERVICE_NAME` 環境変数も使えます。アクセストークンをソース管理に入れないでください。

プログラムが OpenTelemetry Go SDK でトレースも送っている場合、otelprofile は CPU サンプルに、それが実行されたスパンのラベルを付けます。ダッシュボードでは、トレースビューからスパンのプロファイルを開けます。設定は otelprofile の README を参照してください。

## 任意の Linux プロセス: OpenTelemetry eBPF プロファイラー

[OpenTelemetry eBPF プロファイラー](https://github.com/open-telemetry/opentelemetry-ebpf-profiler) は、プログラムを変更せずに、Linux ホスト上のすべてのプロセスの CPU をサンプリングします。Collector のディストリビューション `otelcol-ebpf-profiler` の中で `profiling` レシーバーとして動作し、root 権限（または `CAP_SYS_ADMIN`、`CAP_PERFMON`、`CAP_BPF`）と `/proc` へのアクセスが必要です。

このプロファイラーは各プロセスに `process.executable.name` を付けますが、`service.name` は設定しません。sigiro はサービス名のないプロファイルを拒否します。次の `resource` プロセッサーが、実行ファイル名からサービス名を補います。

```yaml
receivers:
  profiling: {}

processors:
  resource:
    attributes:
      - key: service.name
        from_attribute: process.executable.name
        action: insert

exporters:
  otlp_http:
    endpoint: http://localhost:4318
    # For hosted sigiro:
    # endpoint: https://sigiro.com
    # headers:
    #   Authorization: "Bearer ${env:SIGIRO_ACCESS_TOKEN}"

service:
  pipelines:
    profiles:
      receivers: [profiling]
      processors: [resource]
      exporters: [otlp_http]
```

プロファイルのフィーチャーゲートを有効にして Collector を起動します。

```bash
sudo otelcol-ebpf-profiler --feature-gates=+service.profilesSupport --config config.yaml
```

`otlp_http` という名前より前の Collector リリースでは、同じエクスポーターを `otlphttp` と呼びます。

## OpenTelemetry Collector 経由

すでに OTLP を受け取っている Collector から、プロファイルを sigiro に転送できます。`otlp` レシーバーから `otlp_http` エクスポーターへの `profiles` パイプラインを追加し、`--feature-gates=+service.profilesSupport` を付けて Collector を起動します。

```yaml
receivers:
  otlp:
    protocols:
      http:
      grpc:

exporters:
  otlp_http/sigiro:
    endpoint: http://localhost:4318

service:
  pipelines:
    profiles:
      receivers: [otlp]
      exporters: [otlp_http/sigiro]
```

エクスポーターはエンドポイントに `/v1development/profiles` を付加します。フル URL を指定する必要がある場合は、代わりに `profiles_endpoint` を設定してください。

## HTTP の仕様

- **パス:** `POST /v1development/profiles`。
- **本文:** protobuf の `ExportProfilesServiceRequest`。コンテンツタイプは `application/x-protobuf` です。プロファイルでは JSON を受け付けません。
- **サービス:** すべてのリソースに `service.name` 属性が必要です。ない場合は `400` を返し、何も保存しません。
- **サイズ:** gzip を受け付けます。1 リクエストは既定で展開後 8 MiB までで、それを超えると `413` を返します。
- **エラー:** 不正なプロファイル、または 0 でないサンプルを含まないプロファイルは `400` を返します。Better Auth OAuth/JWT のベアラートークンがない、または無効な場合は `401` を、容量ゲートは `507` を返します。`2xx` のレスポンスは、sigiro がバッチを受け付けたことを意味します。

## 動作を確認する

プロファイラーを少なくとも 1 回のアップロード間隔のあいだ動かします。その後、メインの HTTP API からプロファイルの行を読み返します。

```bash
curl -s -X POST http://localhost:9999/v1/query \
  --data "SELECT service_name, profile_type, unit, count(*) AS samples
          FROM sigiro_profiles
          WHERE service_name = 'checkout'
            AND timestamp > now() - INTERVAL '1 hour'
          GROUP BY 1, 2, 3
          ORDER BY samples DESC"
```

ホスト型 sigiro では、オリジンを変更し、`-H "Authorization: Bearer YOUR_TENANT_KEY"` を追加してください。結果が空でなければ、エクスポート、プロファイルのデコード、HTTP での読み返しが確認できたことになります。ダッシュボードの **Profiles** ページでは、同じデータをフレームグラフで見られます。1 回分の間隔が過ぎても結果が空の場合、たいていはエンドポイントの誤り、クエリのサービス名とプロデューサーの不一致、またはプロデューサーのログに sigiro が読まない形式に対する `400` が出ていることが原因です。

## 次のステップ

- [ホスト型 sigiro へのテレメトリ送信](/docs/how-to/hosted-onboarding) — ホスト型の認証と、その他のテレメトリシグナルを設定します
- [テーブルについて](/docs/explanation/tables) — 各テーブルにどのテレメトリが保存されているかを確認します
