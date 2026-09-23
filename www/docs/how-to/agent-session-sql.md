---
title: Compare Claude Code sessions and rank reported cost with SQL
description: Bounded DuckDB recipes for two Claude Code session IDs and a producer-reported USD leaderboard.
sidebar:
  order: 10
---

These recipes use **Claude Code's native `session.id` on log events**, not a
Sigiro-wide canonical `gen_ai.conversation.id`. They do not identify tasks,
join trace IDs to metrics, or work unchanged for other agents. Inspect actual
`claude_code.api_request` and `claude_code.tool_result` events first: Claude
Code must export logs, and the requested sessions must fit wholly inside the
literal UTC window. The examples below cover September 23, 2026 UTC; replace
both bounds with a finite half-open window around your runs. Select two
**distinct, immutable** native session IDs; an application task ID can help
find them but is not a substitute for a session ID.

The cost is Claude Code's **estimated, producer-reported USD** `cost_usd` on
`claude_code.api_request` log events, not a Sigiro price estimate or a bill.
These are per-request events, not cumulative metric snapshots. Do not also add
`claude_code.cost.usage` metric points: that double-counts the same reported
cost. `input_tokens`, `output_tokens`, `cache_read_tokens`, and
`cache_creation_tokens` are separate dimensions; do not sum them into an
unqualified token total. This event has no reasoning-token field. Auto-mode
permission-check model calls can be absent from these events and cost totals.

## Compare two sessions

Replace only the two ID literals and the two UTC bounds. This returns a single
row with request and tool-event counts, separately reported token dimensions,
priced request coverage, costs, an absolute cost difference, and a percentage
cost difference. A NULL cost is **unavailable**, not zero; percentage change is
NULL when the baseline is missing or zero. A zero event count means _no
matching event in this window_, not proof the run had no activity. If either
session has no matching log rows, investigate the bounds and exporter before
comparing. No completion status, wall-clock or active duration, critical path,
or causal/statistical conclusion is inferred from these events.

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

A pair of runs demonstrates a difference, not statistical significance or
causality. Tool events count completed results rather than tracing each tool
span; the recipe cannot distinguish nesting, retries, or overlapping work.
For complete model/tool additions and removals, inspect grouped request and
result events for each ID rather than hiding absent groups with an inner join.

## Rank session cost

This leaderboard is **not** a cost anomaly detector. It ranks only Claude Code
sessions with at least one API request event in the finite window, up to 50
rows. `priced_requests / requests` shows coverage; sessions with no reported
cost remain visible with NULL cost, last in the ordering. If a session crosses
the window boundary, its amount is partial. No global median or inferred cohort
is appropriate for unrelated tasks, and incomplete sessions are not silently
classified as complete. Use a narrower window if the query reaches the memory
or result limit.

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

## Execute and verify the evidence

Save either SQL block as `comparison.sql` or `leaderboard.sql` and post its raw
contents to `/v1/query` (with your query credential if required):

```bash
curl -sS -D - -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' --data-binary @comparison.sql
```

For MCP, give the **same SQL text** to `sigiro mcp`'s `run_query` tool; no
separate endpoint exists. Check `x-sigiro-truncated` and any query error before
interpreting rows. To inspect a reported request, query `sigiro_logs` with
_the same literal UTC bounds_, `body = 'claude_code.api_request'`, and the
selected `session.id`; include `timestamp`, `log_attributes`, and `trace_id`
for evidence. `trace_id` may be absent on logs; do not invent a trace link.
Only open `/v1/traces/{trace_id}` when a real trace ID was returned. Protect
query access because event attributes can include identity and sensitive content.

These recipes do not provide canonical cross-agent sessions, end-to-end
lifecycle/duration, delegated work, model/tool set diffs, complete cost
attribution, or cohort-relative comparisons. See [agent telemetry](/docs/how-to/agent-telemetry)
and [agent optimization](/docs/how-to/debug-agent-performance) for collection
and diagnosis. Claude Code's [monitoring reference](https://code.claude.com/docs/en/monitoring-usage)
defines the source event fields.
