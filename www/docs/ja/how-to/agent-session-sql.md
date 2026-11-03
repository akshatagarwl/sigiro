---
title: Claude Code セッションの比較と報告コストの順位付け
description: 2 つの Claude Code セッション ID を比較し、提供元が報告した USD コストを順位付けする、範囲を限定した DuckDB のレシピです。
sidebar:
  order: 10
---

これらのレシピは、sigiro 全体で正規化された `gen_ai.conversation.id` ではなく、**ログイベントに付く Claude Code ネイティブの `session.id`** を使います。タスクの識別、トレース ID とメトリクスの結合は行わず、他のエージェントにはそのままでは使えません。まず実際の `claude_code.api_request` と `claude_code.tool_result` イベントを確認してください。Claude Code がログをエクスポートしている必要があり、対象のセッションはリテラルの UTC 時間枠に完全に収まっている必要があります。以下の例は 2026 年 9 月 23 日（UTC）を対象としています。両方の範囲を、実行を囲む有限の半開区間に置き換えてください。**異なる、不変の**ネイティブセッション ID を 2 つ選んでください。アプリケーションのタスク ID は見つける手がかりにはなりますが、セッション ID の代わりにはなりません。

コストは、`claude_code.api_request` ログイベントに付く Claude Code の**推定値で、提供元が報告した USD** の `cost_usd` であり、sigiro による価格の見積もりや請求額ではありません。これらはリクエストごとのイベントで、累積的なメトリクスのスナップショットではありません。`claude_code.cost.usage` メトリクスのデータポイントを加えないでください。同じ報告コストを二重に数えることになります。`input_tokens`、`output_tokens`、`cache_read_tokens`、`cache_creation_tokens` は別々の次元です。これらを合計して限定のないトークン総数にしないでください。このイベントには推論トークンのフィールドはありません。auto モードの権限確認によるモデル呼び出しは、これらのイベントやコスト合計に含まれないことがあります。

## 2 つのセッションを比較する

2 つの ID リテラルと 2 つの UTC 範囲だけを置き換えてください。結果は 1 行で、リクエスト数とツールイベント数、個別に報告されたトークンの各次元、価格付きリクエストのカバー率、コスト、コストの絶対差、コストの差の割合を返します。NULL のコストは 0 ではなく**取得できない**ことを意味します。ベースラインがないか 0 の場合、変化率は NULL になります。イベント数が 0 というのは、_この時間枠に一致するイベントがない_ という意味であり、実行に活動がなかったことの証明ではありません。どちらかのセッションに一致するログ行がない場合は、比較する前に範囲とエクスポーターを調べてください。完了状態、実時間やアクティブな所要時間、クリティカルパス、因果関係や統計的な結論は、これらのイベントからは推論しません。

```sql
SELECT p.baseline_id, p.candidate_id,
       p.baseline_requests, p.candidate_requests,
       p.baseline_priced_requests, p.candidate_priced_requests,
       p.baseline_tool_results, p.candidate_tool_results,
       p.baseline_tool_failures, p.candidate_tool_failures,
       p.baseline_input_tokens, p.candidate_input_tokens,
       p.baseline_output_tokens, p.candidate_output_tokens,
       p.baseline_cache_read_tokens, p.candidate_cache_read_tokens,
       p.baseline_cache_creation_tokens, p.candidate_cache_creation_tokens,
       p.baseline_cost_usd, p.candidate_cost_usd,
       p.candidate_cost_usd - p.baseline_cost_usd AS cost_delta_usd,
       100.0 * (p.candidate_cost_usd - p.baseline_cost_usd)
         / NULLIF(p.baseline_cost_usd, 0) AS cost_delta_pct
FROM (
  SELECT 'baseline-session-id' AS baseline_id,
         'candidate-session-id' AS candidate_id,
         max(requests) FILTER (WHERE session_id = 'baseline-session-id') AS baseline_requests,
         max(requests) FILTER (WHERE session_id = 'candidate-session-id') AS candidate_requests,
         max(priced_requests) FILTER (WHERE session_id = 'baseline-session-id') AS baseline_priced_requests,
         max(priced_requests) FILTER (WHERE session_id = 'candidate-session-id') AS candidate_priced_requests,
         max(tool_results) FILTER (WHERE session_id = 'baseline-session-id') AS baseline_tool_results,
         max(tool_results) FILTER (WHERE session_id = 'candidate-session-id') AS candidate_tool_results,
         max(tool_failures) FILTER (WHERE session_id = 'baseline-session-id') AS baseline_tool_failures,
         max(tool_failures) FILTER (WHERE session_id = 'candidate-session-id') AS candidate_tool_failures,
         max(input_tokens) FILTER (WHERE session_id = 'baseline-session-id') AS baseline_input_tokens,
         max(input_tokens) FILTER (WHERE session_id = 'candidate-session-id') AS candidate_input_tokens,
         max(output_tokens) FILTER (WHERE session_id = 'baseline-session-id') AS baseline_output_tokens,
         max(output_tokens) FILTER (WHERE session_id = 'candidate-session-id') AS candidate_output_tokens,
         max(cache_read_tokens) FILTER (WHERE session_id = 'baseline-session-id') AS baseline_cache_read_tokens,
         max(cache_read_tokens) FILTER (WHERE session_id = 'candidate-session-id') AS candidate_cache_read_tokens,
         max(cache_creation_tokens) FILTER (WHERE session_id = 'baseline-session-id') AS baseline_cache_creation_tokens,
         max(cache_creation_tokens) FILTER (WHERE session_id = 'candidate-session-id') AS candidate_cache_creation_tokens,
         max(cost_usd) FILTER (WHERE session_id = 'baseline-session-id') AS baseline_cost_usd,
         max(cost_usd) FILTER (WHERE session_id = 'candidate-session-id') AS candidate_cost_usd
  FROM (
    SELECT json_extract_string(log_attributes, 'session.id') AS session_id,
           count(*) FILTER (WHERE body = 'claude_code.api_request') AS requests,
           count(TRY_CAST(json_extract_string(log_attributes, 'cost_usd') AS DOUBLE))
             FILTER (WHERE body = 'claude_code.api_request') AS priced_requests,
           count(*) FILTER (WHERE body = 'claude_code.tool_result') AS tool_results,
           count(*) FILTER (WHERE body = 'claude_code.tool_result'
             AND json_extract_string(log_attributes, 'success') = 'false') AS tool_failures,
           sum(TRY_CAST(json_extract_string(log_attributes, 'input_tokens') AS BIGINT))
             FILTER (WHERE body = 'claude_code.api_request') AS input_tokens,
           sum(TRY_CAST(json_extract_string(log_attributes, 'output_tokens') AS BIGINT))
             FILTER (WHERE body = 'claude_code.api_request') AS output_tokens,
           sum(TRY_CAST(json_extract_string(log_attributes, 'cache_read_tokens') AS BIGINT))
             FILTER (WHERE body = 'claude_code.api_request') AS cache_read_tokens,
           sum(TRY_CAST(json_extract_string(log_attributes, 'cache_creation_tokens') AS BIGINT))
             FILTER (WHERE body = 'claude_code.api_request') AS cache_creation_tokens,
           sum(TRY_CAST(json_extract_string(log_attributes, 'cost_usd') AS DOUBLE))
             FILTER (WHERE body = 'claude_code.api_request') AS cost_usd
    FROM sigiro_logs
    WHERE service_name = 'claude-code'
      AND body IN ('claude_code.api_request', 'claude_code.tool_result')
      AND timestamp >= TIMESTAMP '2026-09-23 00:00:00'
      AND timestamp <  TIMESTAMP '2026-09-24 00:00:00'
      AND json_extract_string(log_attributes, 'session.id')
        IN ('baseline-session-id', 'candidate-session-id')
    GROUP BY session_id
  ) AS sessions
) AS p
```

2 回の実行で示せるのは差であって、統計的な有意性や因果関係ではありません。ツールイベントは各ツールのスパンを追跡するのではなく、完了した結果を数えます。このレシピでは、入れ子、リトライ、重なり合う作業を区別できません。モデルやツールの追加と削除をすべて把握するには、内部結合で欠けたグループを隠すのではなく、各 ID についてグループ化したリクエストイベントと結果イベントを調べてください。

## セッションのコストを順位付けする

この順位表はコストの異常検出器では**ありません**。有限の時間枠内に API リクエストイベントが 1 つ以上ある Claude Code セッションだけを、最大 50 行まで順位付けします。`priced_requests / requests` がカバー率を示します。コストが報告されていないセッションも NULL のコストで表示され、並び順の最後になります。時間枠の境界をまたぐセッションの金額は一部だけです。無関係なタスクに全体の中央値や推定したコホートを当てはめるのは適切ではなく、不完全なセッションが黙って完了扱いされることもありません。クエリがメモリや結果の上限に達する場合は、時間枠を狭めてください。

```sql
SELECT json_extract_string(log_attributes, 'session.id') AS session_id,
       count(*) AS requests,
       count(TRY_CAST(json_extract_string(log_attributes, 'cost_usd') AS DOUBLE)) AS priced_requests,
       sum(TRY_CAST(json_extract_string(log_attributes, 'cost_usd') AS DOUBLE)) AS cost_usd,
       sum(TRY_CAST(json_extract_string(log_attributes, 'input_tokens') AS BIGINT)) AS input_tokens,
       sum(TRY_CAST(json_extract_string(log_attributes, 'output_tokens') AS BIGINT)) AS output_tokens,
       sum(TRY_CAST(json_extract_string(log_attributes, 'cache_read_tokens') AS BIGINT)) AS cache_read_tokens,
       sum(TRY_CAST(json_extract_string(log_attributes, 'cache_creation_tokens') AS BIGINT)) AS cache_creation_tokens,
       min(timestamp) AS first_reported_request,
       max(timestamp) AS last_reported_request
FROM sigiro_logs
WHERE service_name = 'claude-code'
  AND body = 'claude_code.api_request'
  AND timestamp >= TIMESTAMP '2026-09-23 00:00:00'
  AND timestamp <  TIMESTAMP '2026-09-24 00:00:00'
  AND json_extract_string(log_attributes, 'session.id') IS NOT NULL
GROUP BY session_id
ORDER BY cost_usd DESC NULLS LAST
LIMIT 50
```

## 実行して根拠を確認する

どちらかの SQL ブロックを `comparison.sql` または `leaderboard.sql` として保存し、その内容をそのまま `/v1/query` に POST します（必要ならクエリ用の認証情報を付けます）。

```bash
curl -sS -D - -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' --data-binary @comparison.sql
```

MCP では、**同じ SQL テキスト**を `sigiro mcp` の `run_query` ツールに渡します。別のエンドポイントはありません。行を解釈する前に、`x-sigiro-truncated` とクエリのエラーを確認してください。報告されたリクエストを調べるには、_同じリテラルの UTC 範囲_、`body = 'claude_code.api_request'`、選んだ `session.id` で `sigiro_logs` をクエリし、根拠として `timestamp`、`log_attributes`、`trace_id` を含めてください。ログには `trace_id` がないことがあります。トレースへのリンクをでっち上げないでください。実際のトレース ID が返された場合にだけ `/v1/traces/{trace_id}` を開いてください。イベントの属性には ID や機微な内容が含まれることがあるため、クエリへのアクセスを保護してください。

これらのレシピは、エージェントをまたいだ正規のセッション、エンドツーエンドのライフサイクルや所要時間、委任された作業、モデルやツールの集合の差分、完全なコスト配分、コホートに対する相対比較は提供しません。収集と診断については [エージェントのテレメトリ](/docs/how-to/agent-telemetry) と [エージェントの最適化](/docs/how-to/debug-agent-performance) を参照してください。元のイベントのフィールドは、Claude Code の [monitoring reference](https://code.claude.com/docs/en/monitoring-usage) で定義されています。
