---
title: Claude Code 세션 비교와 보고된 비용 순위
description: 유한한 UTC 구간에서 두 세션과 생산자가 보고한 USD 비용을 조회하는 SQL.
sidebar:
  order: 10
---

이 방법은 **Claude Code 고유의** 로그 이벤트 속성 `session.id`를 사용합니다. Sigiro 전체의 표준 `gen_ai.conversation.id`가 아니며 다른 에이전트에 그대로 적용할 수 없습니다. `app.task.id`와 같은 외부 작업 ID는 세션을 찾는 데 사용할 수 있지만 비교 대상인 서로 다른 두 개의 불변 세션 ID를 대신하지 않습니다. 로그 내보내기를 활성화하고 실제로 `claude_code.api_request` 및 `claude_code.tool_result` 이벤트가 들어오는지 확인하십시오.

아래에 비교 및 비용 순위 SQL 쿼리 두 개가 있습니다. 둘 다 `2026-09-23 00:00:00` 이상, `2026-09-24 00:00:00` 미만의 **유한한 UTC 반열린 구간**을 사용합니다. 전체 실행을 포함하도록 시간을 교체하고 비교 쿼리의 `baseline-session-id`와 `candidate-session-id`를 실제 ID로 교체하십시오. 구간 밖의 이벤트는 합계에 포함되지 않습니다.

## 두 세션 비교

비교 쿼리는 API 요청 수, 비용이 기록된 요청 수, 완료된 도구 결과와 실패 수, 입력·출력·캐시 읽기·캐시 생성 토큰, USD 비용, 비용 차이와 변화율을 반환합니다. 비용은 `claude_code.api_request`의 `cost_usd`로, Claude Code가 계산한 **추정 USD**이며 청구 금액이 아닙니다. 누적 메트릭 스냅샷을 합산하지 않습니다. 같은 비용을 보고하는 `claude_code.cost.usage`까지 더하면 이중 계산입니다. 토큰 항목을 의미 없는 단일 총계로 합치지 마십시오. 이 이벤트에는 추론 토큰 항목이 없습니다.

비용 또는 토큰 값이 누락되면 `NULL`로 남기고 0으로 바꾸지 않습니다. 기준 비용이 0이거나 없으면 변화율은 `NULL`입니다. 이벤트 수 0은 지정한 기간에 일치하는 이벤트가 없다는 뜻일 뿐 활동이 없었다는 증거가 아닙니다. 완료 상태, 경과 시간, 활성 시간, 위임 작업의 중요 경로는 이 쿼리로 구하지 않습니다. 두 실행의 차이는 통계적 유의성이나 인과관계를 입증하지 않습니다.

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

## 세션 비용 순위

순위표는 기간 내 API 요청 이벤트가 있는 Claude Code 세션을 최대 50개 보여주고 추정 USD 비용의 내림차순으로 정렬합니다. 비용이 없는 세션도 `NULL` 상태로 맨 뒤에 남습니다. `priced_requests / requests`로 비용 자료의 적용 범위를 확인하십시오. 기간 경계에 걸친 세션의 금액은 부분 합계입니다. 서로 다른 작업을 한 전역 중앙값으로 비교해 이상 비용이라고 판정하지 마십시오. 이 순위가 낭비나 원인의 증거는 아닙니다. 명시적인 코호트별 비교와 세션 완료 판정은 아직 지원하지 않습니다.

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

## 실행과 근거 확인

위의 SQL 블록을 `comparison.sql`과 `leaderboard.sql`로 저장하고 `curl -X POST http://127.0.0.1:9999/v1/query -H 'content-type: text/plain' --data-binary @comparison.sql`처럼 보냅니다. MCP에서는 동일한 SQL을 `sigiro mcp`의 `run_query`로 전달하십시오. `x-sigiro-truncated` 응답 헤더와 오류를 확인하십시오. 각 요청의 근거는 동일한 리터럴 UTC 구간, 선택한 `session.id`, `body = 'claude_code.api_request'`를 사용하여 `sigiro_logs`에서 조회합니다. 실제 `trace_id`가 없는 경우 트레이스 링크를 만들어 내지 마십시오. 로그에 신원 정보가 포함될 수 있으므로 쿼리 접근을 제한하십시오.

이 SQL은 여러 에이전트의 표준 세션, 수명주기, 모델·도구 추가 및 제거, 완전한 비용 귀속, 코호트 중앙값을 제공하지 않습니다. [에이전트 텔레메트리](/docs/ko/how-to/agent-telemetry)도 참고하십시오.
