---
title: Jaeger と並行して sigiro を試す
description: アプリを切り替えず、Jaeger v2 のトレースをローカルの sigiro にも送る評価手順。
sidebar:
  order: 8
---

これは移行ではなく、**トレースだけ**の評価です。アプリは従来どおり Jaeger v2 の OTLP レシーバー (`4317`/`4318`) に送信します。Jaeger のトレースパイプラインに二つ目のエクスポーターを追加し、既存のストレージエクスポーターと他のパイプラインは残します。Jaeger はログとメトリクスを受け付けません。Jaeger の前段に別の OpenTelemetry Collector がある場合は、その Collector のトレースパイプラインに同じ変更を加え、従来の Jaeger 宛ての出力は維持します。

## 1. ホスト側ポートを変更して起動

Jaeger と**同じホスト**で `14317`、`14318`、`9999` が空いていることを確認します。

```bash
docker run -d --name sigiro-trial --restart unless-stopped \
  -p 127.0.0.1:14317:4317 \
  -p 127.0.0.1:14318:4318 \
  -p 127.0.0.1:9999:9999 \
  -v sigiro-trial-data:/var/lib/sigiro \
  ghcr.io/sigiroai/sigiro
curl --fail http://127.0.0.1:9999/health
```

`14317` は gRPC 用ですが、この例では HTTP の `14318` だけを使用します。`9999` が使用中ならホスト側も `19999` などに変更し、以下の確認にもそのポートを使用してください。ヘルスチェックが失敗したら `docker logs sigiro-trial` を調べます。アプリの送信先は変更しません。

**セキュリティ:** この `127.0.0.1` は Collector と sigiro が同じホストのネットワーク名前空間から到達できる場合だけ有効です。別コンテナや別ホストの Collector からはホストのループバックに接続できません。到達可能なプライベート経路と TLS/認証を用意してください。安易に `0.0.0.0` に公開しないでください。セルフホスト sigiro の OTLP/クエリエンドポイントは認証しません。信頼できないネットワークに直接公開せず、ローカルプロセスも無条件には信頼しないでください。

## 2. 既存の設定にミラーを追加

元の YAML をバックアップし、以下の**断片をマージ**します。既存のレシーバー、プロセッサー、エクスポーター、Jaeger ストレージ、ログ/メトリクスのパイプラインと順序は維持します。この例は Jaeger 2.0.0 のコンポーネント名 `otlphttp` を使います。新しい Collector では `otlp_http` と呼び、古い名前は非推奨の別名です。使用中のバイナリで検証し、リストと定義に同じ名前を使ってください。

```yaml
exporters:
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
      # 実際の設定の receiver と processor を維持すること。
      receivers: [otlp]
      processors: [batch]
      exporters: [jaeger_storage_exporter, otlphttp/sigiro]
```

`endpoint` はベース URL です。OTLP HTTP エクスポーターが `/v1/traces` を追加します。ここにパスを重複して付けないでください。平文 HTTP は同一ホストのループバックに限定し、リモートでは TLS と認証を設定します。キューはメモリ内の最大 **1000 リクエスト**であり、1000 スパンではありません。5 分後には再試行が終了し、キュー満杯、再起動、送信失敗時にはミラー側のデータを失う可能性があります。**無損失は保証されません。** Collector のキュー使用量・容量、送信失敗・キュー投入失敗メトリクス、および Jaeger の取り込みエラーを監視してください。異常が増えたらロールバックします。Jaeger のログ/メトリクスパイプラインには追加しません。

## 3. 既存環境の再起動前に検証

編集したファイルに対し、**同じバージョン**の Jaeger v2/Collector バイナリで `validate --config=<編集したファイル>` を実行します。これは構文・設定の検証であり、到達性や配信の確認ではありません。次に編集済み設定のコピーで独立した試験インスタンスを起動します。コピー内の**すべての**リスナーを未使用ポートに変更してください。OTLP `4317`/`4318`、クエリ UI `16686`、Prometheus `8888`（使用時は `8889`）、ヘルス `13133`、その他の拡張機能・レシーバー・クエリリスナーも対象です。試験ストレージは本番から隔離し、本番ストレージへの書き込みを禁止します。両方の試験送信先でトレースを確認してから通常の手順で本番設定を反映してください。Docker の `-p` 変更だけでは共有名前空間内のリスナーやストレージ衝突を防げません。隔離できない場合、本番再起動を最初の構文確認にしないでください。

## 4. 両方を確認

**元の**アプリまたは OTLP 送信先経由で識別しやすい新しいスパンを生成し、trace ID、サービス、時刻を記録します。Jaeger UI（通常 `16686`）でその ID のスパンを確認します。sigiro では `curl --fail http://127.0.0.1:9999/v1/services` でサービスを確認し、同じ ID を検索します。テスト時刻を含む UTC の開始・終了時刻を具体的な値に置き換えてください。`now()` は DuckLake のファイル絞り込みを妨げ、クエリの時間境界ポリシーにも適合しません。

```bash
curl --fail -X POST http://127.0.0.1:9999/v1/query \
  --data "SELECT trace_id, span_name, timestamp FROM sigiro_spans
          WHERE trace_id = '<your-trace-id>'
            AND timestamp >= TIMESTAMP '<start-utc:YYYY-MM-DD HH:MM:SS>'
            AND timestamp < TIMESTAMP '<end-utc:YYYY-MM-DD HH:MM:SS>'"
```

いずれかに存在しない場合は Jaeger/Collector のログ、キュー・送信失敗メトリクス、sigiro のログ、ネットワーク名前空間、URL を調べます。件数だけでなく trace ID を照合してください。上流のサンプリング/フィルタリングにも注意します。

## ロールバック

バックアップした YAML に戻します（`traces.exporters` のリストと `exporters` マップの**両方**から `otlphttp/sigiro` を削除）。検証して通常の手順で再起動し、新しいトレースが Jaeger に入ることを確認します。その後、試験コンテナを削除します。

```bash
docker rm -f sigiro-trial
# 試験データの廃棄を決めた場合のみ（元に戻せません）:
docker volume rm sigiro-trial-data
```

アプリの設定や Jaeger の既存ストレージは変更しません。データを残す場合は最後のコマンドを省いてください。

設定の出典: [Collector の設定と検証](https://opentelemetry.io/docs/collector/configuration/)、[OTLP HTTP exporter](https://github.com/open-telemetry/opentelemetry-collector/blob/v0.112.0/exporter/otlphttpexporter/README.md)、[キューと再試行](https://github.com/open-telemetry/opentelemetry-collector/blob/v0.112.0/exporter/exporterhelper/README.md)、[トラブルシューティング](https://opentelemetry.io/docs/collector/troubleshooting/)、[Jaeger v2 API](https://www.jaegertracing.io/docs/2.0/architecture/apis/)。
