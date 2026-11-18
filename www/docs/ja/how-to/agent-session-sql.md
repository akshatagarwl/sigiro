---
title: Claude Code セッションの比較と報告コストの順位付け
description: 2 つのセッションを比較し、報告された USD コストを有限の UTC 範囲で順位付けする SQL。
sidebar:
  order: 10
---

この手順は **Claude Code 固有の**ログイベント属性 `session.id` を使用します。Sigiro 全体の正規 ID `gen_ai.conversation.id` ではなく、他のエージェントにそのまま適用することもできません。`app.task.id` などの外部タスク ID はセッションを探す手掛かりになりますが、比較する 2 つの異なる不変セッション ID の代わりにはなりません。まずログエクスポートを有効にし、実際に `claude_code.api_request` と `claude_code.tool_result` が届いていることを確認してください。

以下に比較とコスト順位付けの SQL クエリを 2 本示します。両方とも `2026-09-23 00:00:00` 以上、`2026-09-24 00:00:00` 未満の **UTC の有限な半開区間**を使います。実際の実行全体を含む時刻に置き換え、比較クエリでは `baseline-session-id` と `candidate-session-id` を実際の ID に置き換えてください。時刻の範囲外のイベントは合計に含まれません。

## 2 セッションの比較

比較クエリは API リクエスト数、費用が記録されたリクエスト数、完了したツール結果と失敗数、入力・出力・キャッシュ読み取り・キャッシュ作成トークン、USD 費用、費用差と変化率を返します。費用は `claude_code.api_request` イベントの `cost_usd`、つまり Claude Code が算出した**推定 USD**であり、請求額ではありません。累積メトリクスのスナップショットを足していません。同じ費用を記録する `claude_code.cost.usage` と合わせて加算しないでください。異なるトークンの内訳を無条件で一つの合計にまとめないでください。推論トークンはこのイベントから取得できません。

費用やトークン属性が記録されていなければ結果は `NULL` です。ゼロに置き換えません。基準となる費用がゼロまたは欠落していれば変化率も `NULL` です。イベント数ゼロはこの期間に一致するイベントがないことのみを意味し、活動がなかった証拠ではありません。完了状態、経過時間、アクティブ時間、委譲作業のクリティカルパスはこのクエリでは求められません。2 回の実行差は統計的有意性や因果関係を示しません。

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

## セッション費用ランキング

順位表は期間内に API リクエストのあった Claude Code セッションを最大 50 件表示し、推定 USD 費用の降順に並べます。費用が記録されていないセッションも `NULL` のまま末尾に残ります。`priced_requests / requests` で記録の網羅性を確認してください。期間をまたぐセッションは費用の一部しか表示されません。無関係なタスクの全体中央値を異常判定の基準にせず、この順位を「無駄」や原因の証明と解釈しないでください。コホート相対の比較や完了状態の判定は未対応です。

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

## 実行と証拠

上記の SQL ブロックをそれぞれ `comparison.sql`、`leaderboard.sql` に保存し、`curl -X POST http://127.0.0.1:9999/v1/query -H 'content-type: text/plain' --data-binary @comparison.sql` のように送信します。MCP では同じ SQL 本文を `sigiro mcp` の `run_query` に渡します。結果の `x-sigiro-truncated` とエラーを確認してください。個別の証拠は同じリテラル UTC 範囲、選択した `session.id`、`body = 'claude_code.api_request'` で `sigiro_logs` を再検索します。実際に `trace_id` が返るまではトレースへのリンクを作らないでください。ログには個人情報が含まれ得るので、クエリアクセスを制限してください。

この SQL では正規のエージェント共通セッション、ライフサイクル、モデル・ツールの追加削除、費用帰属の完全性、コホート別中央値は扱いません。[エージェントのテレメトリ](/docs/ja/how-to/agent-telemetry)も参照してください。
