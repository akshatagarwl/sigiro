---
title: 比较 Claude Code 会话并按已报告费用排序
description: 在有限的 UTC 时间范围内查询两个会话及生产者报告的美元费用。
sidebar:
  order: 10
---

此方法使用 **Claude Code 特有的**日志事件属性 `session.id`，而不是 Sigiro 跨 agent 的规范 `gen_ai.conversation.id`；不能直接应用于其他 agent。外部任务 ID（例如 `app.task.id`）可以帮助定位会话，却不能替代用于比较的两个不同且不变的会话 ID。先启用日志导出，确认实际收到 `claude_code.api_request` 和 `claude_code.tool_result` 事件。

下面提供两条可执行的比较与费用排行 SQL 查询。两者都使用 **UTC 有限半开区间**：从 `2026-09-23 00:00:00`（含）到 `2026-09-24 00:00:00`（不含）。将两个界限替换为涵盖整个实际执行的时间；比较查询还需将 `baseline-session-id` 和 `candidate-session-id` 替换为真实 ID。区间外的事件不会进入合计。

## 比较两个会话

比较查询返回 API 请求数、具有费用的请求数、已完成的工具结果及失败数、输入／输出／缓存读取／缓存创建 token、美元费用、费用绝对差与百分比变化。费用来自 `claude_code.api_request` 事件中的 `cost_usd`，是 Claude Code **估算并报告的美元费用**，不是实际账单。这里不累加累积指标快照；再加上记录相同费用的 `claude_code.cost.usage` 会造成重复计算。不同 token 维度不能不加说明地合成一个总量。此事件不提供推理 token。

未报告的费用或 token 是 `NULL`，不是零。基准费用为零或缺失时，百分比变化也是 `NULL`。事件数为零仅表示该时间范围内没有匹配事件，并不能证明会话没有活动。这条查询无法得出完成状态、实际经过时间、活跃时间或委派工作的关键路径。两次执行出现差异并不证明统计显著性或因果关系。

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

## 按会话费用排序

排行榜最多返回该区间内有 API 请求事件的 50 个 Claude Code 会话，按估算美元费用降序排列。没有费用的会话仍以 `NULL` 保留在末尾。通过 `priced_requests / requests` 检查费用覆盖情况。跨越时间界限的会话只会得到部分费用。不要将互不相同的任务与一个全局中位数比较并自动认定为异常；费用排名不是浪费或原因的证据。显式同类群组比较以及完成状态判定尚未包含。

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

## 执行与核对证据

将上面的两个 SQL 代码块保存为 `comparison.sql` 和 `leaderboard.sql`，然后使用 `curl -X POST http://127.0.0.1:9999/v1/query -H 'content-type: text/plain' --data-binary @comparison.sql` 发送原始 SQL。对于 MCP，将相同的 SQL 交给 `sigiro mcp` 的 `run_query`。检查 `x-sigiro-truncated` 响应头及错误。查看单次请求的证据时，使用相同的字面量 UTC 界限、选中的 `session.id` 和 `body = 'claude_code.api_request'` 查询 `sigiro_logs`。只有实际返回 `trace_id` 才能链接到对应 trace，不能编造。日志可能包含身份和敏感信息，应限制查询权限。

这些 SQL 不提供跨 agent 的规范会话、生命周期、模型及工具的增加／删除、完整的费用归属或同类群组中位数。另见[agent 遥测](/docs/zh/how-to/agent-telemetry)。
