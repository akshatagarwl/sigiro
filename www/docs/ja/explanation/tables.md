---
title: テレメトリテーブルについて
description: 各 sigiro_* テーブルの内容と、テレメトリを送信したマシンの調べ方。
sidebar:
  order: 2
---

sigiro は OTLP で送信されたテレメトリを保存します。各行が表すのは、そのテレメトリを送信したサービスとマシンです。Sigiro サーバー自身が動作するマシンのライブなホスト情報は公開しません。

## 保存されるシグナル

- `sigiro_spans` — トレースとスパン時間
- `sigiro_logs` — ログレコード
- `sigiro_metrics_gauge` — 時点値
- `sigiro_metrics_sum` — 単調・非単調の合計
- `sigiro_metrics_histogram` — 明示的バケットのヒストグラム
- `sigiro_metrics_exp_histogram` — 指数ヒストグラム
- `sigiro_profiles` — プロファイルサンプル
- `sigiro_anomalies` — 検出されたレジームシフト

OTLP が異なる 4 種類のメトリクス形状を定義するため、メトリクスは 4 テーブルに分かれています。複数種類を同時に読む場合は `UNION ALL` を使います。

## 正しいマシンについて調べる

CPU、メモリ、ファイルシステム、ネットワークは `sigiro_metrics_*` を問い合わせます。結果は Sigiro の実行マシンではなく、メトリクスを送信したマシンについての値です。

```sql
SELECT service_name, metric_name, value, timestamp
FROM sigiro_metrics_gauge
WHERE service_name = 'checkout'
  AND metric_name LIKE 'system.filesystem%'
  AND timestamp >= TIMESTAMP '2026-09-12 00:00:00'
  AND timestamp <  TIMESTAMP '2026-09-13 00:00:00'
ORDER BY timestamp DESC;
```

結果が空なら、そのサービスは該当するホストメトリクスを送信していません。別のマシンの値で代用せず、計装を修正してください。

## クエリ規則

生のテレメトリクエリには有限の半開時間範囲、`timestamp >= ... AND timestamp < ...` が必要です。

## 次に読む

- [ダッシュボードではなく証拠について](/ja/docs/explanation/evidence)
- [ホスト型 sigiro にテレメトリを送信する](/ja/docs/how-to/hosted-onboarding#4-query-your-data-sql-api)
