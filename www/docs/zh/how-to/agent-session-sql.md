---
title: 比较 Claude Code 会话并按已报告费用排序
description: 针对两个 Claude Code 会话 ID 的有界 DuckDB 查询配方，以及按生产方报告的美元费用排序的排行榜。
sidebar:
  order: 10
---

这些配方使用的是**日志事件上 Claude Code 原生的 `session.id`**，而不是 sigiro 范围内规范化的 `gen_ai.conversation.id`。它们不识别任务，不把 trace ID 与指标关联，也不能原样用于其他智能体。请先查看实际的 `claude_code.api_request` 和 `claude_code.tool_result` 事件：Claude Code 必须导出日志，并且所请求的会话必须完全落在 UTC 字面时间窗口内。下面的示例覆盖 2026 年 9 月 23 日 UTC；请把两个边界都替换为覆盖你运行的有限半开窗口。选择两个**不同且不可变**的原生会话 ID；应用的任务 ID 可以帮助找到它们，但不能替代会话 ID。

费用是 `claude_code.api_request` 日志事件上 Claude Code **估算的、由生产方报告的美元** `cost_usd`，不是 sigiro 的价格估算，也不是账单。这些是按请求的事件，不是累计的指标快照。不要再额外加上 `claude_code.cost.usage` 指标数据点：那会把同一笔报告费用算两遍。`input_tokens`、`output_tokens`、`cache_read_tokens` 和 `cache_creation_tokens` 是不同的维度；不要把它们加成一个不加限定的 token 总数。该事件没有推理 token 字段。auto 模式权限检查产生的模型调用可能不在这些事件和费用总计中。

## 比较两个会话

只替换两个 ID 字面量和两个 UTC 边界。它返回一行结果，包含请求数和工具事件数、分别报告的各个 token 维度、带价格请求的覆盖率、费用、费用的绝对差，以及费用差的百分比。NULL 费用表示**不可用**，而不是零；基线缺失或为零时，百分比变化为 NULL。事件数为零表示*该窗口内没有匹配的事件*，并不能证明这次运行没有任何活动。如果任一会话没有匹配的日志行，请在比较之前检查边界和导出器。这些事件不推断完成状态、实际耗时或活跃时长、关键路径，也不推断因果或统计结论。

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

两次运行展示的是差异，而不是统计显著性或因果关系。工具事件统计的是已完成的结果，而不是追踪每个工具 span；该配方无法区分嵌套、重试或重叠的工作。要完整了解模型/工具的增减，请查看每个 ID 分组后的请求和结果事件，而不要用内连接把缺失的分组藏起来。

## 按会话费用排序

这个排行榜**不是**费用异常检测器。它只对有限窗口内至少有一个 API 请求事件的 Claude Code 会话排序，最多 50 行。`priced_requests / requests` 显示覆盖率；没有报告费用的会话仍然可见，费用为 NULL，排在最后。如果某个会话跨越了窗口边界，它的金额就是不完整的。对不相关的任务使用全局中位数或推断的群组并不合适，不完整的会话也不会被悄悄归类为完整。如果查询达到内存或结果上限，请使用更窄的窗口。

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

## 执行并核对证据

把任一 SQL 块保存为 `comparison.sql` 或 `leaderboard.sql`，并把其原始内容 POST 到 `/v1/query`（如有需要，附上查询凭据）：

```bash
curl -sS -D - -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' --data-binary @comparison.sql
```

使用 MCP 时，把**同样的 SQL 文本**交给 `sigiro mcp` 的 `run_query` 工具；不存在单独的端点。在解读结果行之前，请检查 `x-sigiro-truncated` 和任何查询错误。要查看某个已报告的请求，请用*相同的 UTC 字面边界*、`body = 'claude_code.api_request'` 和所选的 `session.id` 查询 `sigiro_logs`；把 `timestamp`、`log_attributes` 和 `trace_id` 包含进来作为证据。日志上可能没有 `trace_id`；不要凭空捏造链路关联。只有在确实返回了 trace ID 时才打开 `/v1/traces/{trace_id}`。由于事件属性可能包含身份信息和敏感内容，请保护好查询访问权限。

这些配方不提供跨智能体的规范会话、端到端的生命周期/时长、委派的工作、模型/工具集合的差异、完整的费用归因，也不提供相对于群组的比较。采集和诊断请参阅[智能体遥测](/docs/how-to/agent-telemetry)和[智能体优化](/docs/how-to/debug-agent-performance)。源事件字段由 Claude Code 的 [monitoring reference](https://code.claude.com/docs/en/monitoring-usage) 定义。
