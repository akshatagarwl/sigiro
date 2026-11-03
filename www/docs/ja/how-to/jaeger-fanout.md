---
title: Jaeger と並行して sigiro を試す
description: アプリケーションの送信先を変えずに、既存の Jaeger v2 Collector からループバック専用の sigiro インスタンスへ OTLP トレースをミラーします。
sidebar:
  order: 8
---

これは移行ではなく、**トレースだけの評価**です。アプリケーションは引き続き、`4317`/`4318` の既存の Jaeger v2 OTLP レシーバーに送信させてください。そのレシーバーの **traces** パイプラインに sigiro を 2 つ目のエクスポーターとして追加し、Jaeger のストレージエクスポーターと他のすべてのパイプラインはそのまま残します。Jaeger が受け付けるのはトレースデータで、OTLP のログやメトリクスは受け付けません。Jaeger の前段に別の OpenTelemetry Collector がある場合は、既存の Jaeger の送信先を残したまま、同じ traces パイプラインの変更をそちらで行ってください。

## 1. 別のホストポートで sigiro を起動する

Jaeger と**同じホスト**で、`14317`、`14318`、`9999` が空いている状態で実行します。

```bash
docker run -d --name sigiro-trial --restart unless-stopped \
  -p 127.0.0.1:14317:4317 \
  -p 127.0.0.1:14318:4318 \
  -p 127.0.0.1:9999:9999 \
  -v sigiro-trial-data:/var/lib/sigiro \
  ghcr.io/sigiroai/sigiro
curl --fail http://127.0.0.1:9999/health
```

`14317` は付け替えた gRPC ポートで、このガイドでは `14318` の OTLP HTTP だけを使います。`9999` が使用中の場合は、その**ホスト**側も付け替え（例: `-p 127.0.0.1:19999:9999`）、以下の確認ではそのポートを使ってください。ヘルスチェックが成功するまで待つか、`docker logs sigiro-trial` を確認してください。アプリケーションのエクスポーターのエンドポイントは変更しないでください。

**ネットワークの境界:** この例は、Jaeger/Collector が自身のネットワーク名前空間からホストの `127.0.0.1:14318` に到達できることを前提としています。別のコンテナや別のホストにある Collector は、そのループバックアドレスを使えません。TLS や認証を行うプロキシ（または同等のネットワーク制御）を通るプライベートな経路を用意し、到達可能なそのアドレスを使ってください。sigiro 自体は認証を提供しません。これを動かすためだけに Docker のバインドを `0.0.0.0` に変えないでください。
セルフホストの sigiro は OTLP やクエリのリクエストを認証しません。これらのポートを信頼できないネットワークに直接公開しないでください。ループバックへのバインドだけでは、ローカルのプロセスが信頼できることにはなりません。

## 2. 既存の traces パイプラインに上限付きのミラーを追加する

Jaeger v2/Collector の YAML をバックアップしてください。次のエントリを既存の設定に**マージ**します。これは断片であり、置き換え用の設定ではありません。既存のレシーバー、プロセッサー、エクスポーター、拡張機能、Jaeger のストレージ、トレース以外のパイプラインは変更しないでください。traces パイプラインに他のエクスポーターやプロセッサーがある場合は、既存の順序のまま残してください。この例では Jaeger **2.0.0** のコンポーネント ID を使っています。このリリースでは OTLP HTTP エクスポーターを `otlphttp` と呼びます。新しい Collector ディストリビューションでは `otlp_http` と呼びます（古い綴りは非推奨のエイリアスです）。インストール済みのバイナリで `validate` を実行して確認し、受け付けられた名前を両方の箇所で一貫して使ってください。

```yaml
exporters:
  # Add alongside the existing jaeger_storage_exporter; do not replace it.
  otlphttp/sigiro:
    endpoint: http://127.0.0.1:14318
    sending_queue:
      enabled: true
      queue_size: 1000
    retry_on_failure:
      enabled: true
      initial_interval: 5s
      max_interval: 30s
      max_elapsed_time: 300s

service:
  pipelines:
    traces:
      # Preserve the actual receiver and processor lists from your config.
      receivers: [otlp]
      processors: [batch]
      exporters: [jaeger_storage_exporter, otlphttp/sigiro]
```

`endpoint` は OTLP/HTTP の**ベース** URL で、エクスポーターが `/v1/traces` を付加します。ここに `/v1/traces` を加えたり、フルパスなしでシグナルごとの `traces_endpoint` を使ったりしないでください。ここでの平文 HTTP は、同一ホストのループバックの場合にのみ適切です。`http://` のエンドポイントには `tls: { insecure: true }` は不要です。リモートのプライベートな送信先では、代わりに両側で TLS と認証を設定してください。

キューは**メモリ上**にあり、保持するのは最大 1,000 スパンではなく最大 1,000 リクエストです。リトライはバッチごとに 5 分で終了します。キューが満杯になったり、リトライが期限切れになったり、Collector が再起動したりすると、ミラーされたスパンが失われることがあります。損失がないことを**保証する**ファンアウトでは**ありません**。Collector のエクスポーターキューの容量とサイズ、送信失敗とエンキュー失敗のメトリクスを監視し、負荷とメモリに見合ったキューサイズにしてください。これらの失敗や Jaeger のインジェストエラーが増えた場合は、試験を中止して元の設定に戻してください。Jaeger のメトリクスやログのパイプラインに sigiro のエクスポーターを追加しないでください。

## 3. 本番の構成を変える前に検証する

まず、編集したファイルに対して、**同じバージョンの Jaeger v2 または Collector のバイナリ／イメージ**で `validate --config=<edited-file>` コマンドを実行します。検証で確認されるのは設定であり、ネットワークアクセス、配送、ストレージでは**ありません**。次に、編集した設定のコピーを使って、同じバージョンの別のスクラッチインスタンスを起動します。スクラッチ用のコピーでは、OTLP の `4317`/`4318`、Jaeger のクエリ／UI の `16686`、内部 Prometheus メトリクスの `8888`（および `8889` があればそれも）、ヘルスの `13133`、その他の有効な拡張機能、レシーバー、クエリのリスナーを含め、**すべての**リスナーを未使用のポートに付け替えてください。ストレージは本番から分離するか、読み取り専用のテスト用バックエンドを使ってください。スクラッチのエクスポーターに本番のストレージへ書き込ませないでください。本番のインスタンスを再起動する前に、スクラッチのプロセスが起動し、テスト用トレースを両方のスクラッチの送信先に送れることを確認してください。Docker の `-p` による付け替えだけでは、共有ネットワーク名前空間内のリスナーは変わらず、ストレージの衝突も防げ**ません**。スクラッチの分離が用意できない場合は、本番の再起動を最初の構文テストにしないでください。

スクラッチでの確認が通ったら、通常の管理された Jaeger/Collector の再起動またはロールアウトで、編集した設定を適用します。すぐに起動ログと既存の Jaeger クエリ UI を確認してください。ロールバック用に元の YAML を残しておいてください。

## 4. 両方のバックエンドを確認する

**既存の**アプリ、またはその元の OTLP エンドポイントを通じて（sigiro に直接ではなく）、一意な名前の**新しい**テスト用スパンを生成します。トレース ID、サービス名、時刻を記録してください。バッチ／エクスポートの遅延の後に:

1. 既存の Jaeger UI（通常はポート `16686`）でそのトレース ID を探し、スパンがあることを確認します。これで既存のストレージ経路が引き続き動いていることが証明されます。Collector の起動成功だけでは証明になりません。
2. sigiro では、`curl --fail http://127.0.0.1:9999/v1/services` でサービスを確認します。`POST /v1/query` を使ってテストの時間枠内でその正確なトレース ID をクエリするか、対応するスパンを探します。たとえば、トレース ID とテスト用スパンを囲む UTC の時間枠を置き換えて実行します。リテラルの範囲を使ってください。`now()` は DuckLake のファイル絞り込みを妨げることがあり、範囲限定クエリのポリシーは半開区間のリテラルを必要とします。

   ```bash
   curl --fail -X POST http://127.0.0.1:9999/v1/query \
     --data "SELECT trace_id, span_name, timestamp FROM sigiro_spans
             WHERE trace_id = '<your-trace-id>'
               AND timestamp >= TIMESTAMP '<start-utc:YYYY-MM-DD HH:MM:SS>'
               AND timestamp < TIMESTAMP '<end-utc:YYYY-MM-DD HH:MM:SS>'"
   ```

どちらかのバックエンドにスパンがない場合は、結論を出す前に、Jaeger/Collector のログ、エクスポーターの失敗／キューのメトリクス、sigiro のログ、ネットワーク名前空間、エンドポイントを調べてください。合計件数だけでなく、実際のトレース ID を比較してください。上流でのサンプリングやフィルタリングは両方の送信先に影響することがあります。

## ロールバック

バックアップした YAML を復元し（`otlphttp/sigiro` を **traces のエクスポーター一覧とエクスポーターのマップの両方から**削除）、検証してから、同じ管理された再起動／ロールアウトを行います。Jaeger の元のレシーバー、ストレージエクスポーター、アプリケーションの設定、その他のパイプラインはそのまま残ります。Jaeger で新しいトレースを確認してください。その後、試験用インスタンスを停止して削除します。

```bash
docker rm -f sigiro-trial
# Only after deciding to discard the trial's stored telemetry:
docker volume rm sigiro-trial-data
```

ボリュームの削除は元に戻せません。試験データを残す場合は最後のコマンドを省略してください。ロールバックの一環として Jaeger を停止したり、アプリケーションの送信先を変えたりしないでください。

## 設定のリファレンス

- [Collector configuration and validation](https://opentelemetry.io/docs/collector/configuration/)
- [Jaeger 2.0.0 exporter factory](https://github.com/jaegertracing/jaeger/blob/v2.0.0/cmd/jaeger/internal/components.go) と [Collector 0.112.0 component ID](https://github.com/open-telemetry/opentelemetry-collector/blob/v0.112.0/exporter/otlphttpexporter/internal/metadata/generated_status.go)
- [OTLP HTTP exporter settings (Collector 0.112.0)](https://github.com/open-telemetry/opentelemetry-collector/blob/v0.112.0/exporter/otlphttpexporter/README.md)
- [Exporter queue/retry behavior (Collector 0.112.0)](https://github.com/open-telemetry/opentelemetry-collector/blob/v0.112.0/exporter/exporterhelper/README.md)
- [Collector troubleshooting and exporter metrics](https://opentelemetry.io/docs/collector/troubleshooting/)
- [Jaeger v2 API/port reference](https://www.jaegertracing.io/docs/2.0/architecture/apis/)
