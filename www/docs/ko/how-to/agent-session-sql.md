---
title: Claude Code 세션 비교와 보고된 비용 순위
description: Claude Code 세션 ID 두 개를 비교하고, 생산자가 보고한 USD 비용의 순위를 매기는 범위가 정해진 DuckDB 레시피입니다.
sidebar:
  order: 10
---

이 레시피는 sigiro 전체의 정규 `gen_ai.conversation.id`가 아니라 **로그 이벤트에 있는 Claude Code 고유의 `session.id`**를 사용합니다. 작업을 식별하거나, 트레이스 ID를 메트릭과 조인하거나, 다른 에이전트에서 그대로 동작하지 않습니다. 먼저 실제 `claude_code.api_request`와 `claude_code.tool_result` 이벤트를 살펴보십시오. Claude Code가 로그를 내보내야 하며, 요청한 세션은 리터럴 UTC 구간 안에 완전히 들어가야 합니다. 아래 예시는 2026년 9월 23일 UTC를 다룹니다. 두 범위를 실행을 감싸는 유한한 반열린 구간으로 바꾸십시오. **서로 다르고 변하지 않는** 고유 세션 ID 두 개를 고르십시오. 애플리케이션 작업 ID는 그것을 찾는 데 도움이 될 수 있지만 세션 ID를 대신하지는 못합니다.

비용은 `claude_code.api_request` 로그 이벤트에 있는 Claude Code의 **추정치이자 생산자가 보고한 USD** `cost_usd`이며, sigiro의 가격 추정이나 청구서가 아닙니다. 이것은 요청별 이벤트이며 누적 메트릭 스냅샷이 아닙니다. `claude_code.cost.usage` 메트릭 포인트를 함께 더하지 마십시오. 같은 보고 비용을 두 번 세게 됩니다. `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_creation_tokens`는 별개의 차원이므로 한정 없는 토큰 합계로 더하지 마십시오. 이 이벤트에는 추론 토큰 필드가 없습니다. auto 모드 권한 확인용 모델 호출은 이 이벤트와 비용 합계에서 빠질 수 있습니다.

## 두 세션 비교하기

두 ID 리터럴과 두 UTC 범위만 바꾸십시오. 요청 수와 도구 이벤트 수, 따로 보고된 토큰 차원, 가격이 매겨진 요청의 비율, 비용, 비용의 절대 차이, 비용 차이의 백분율이 담긴 한 행을 반환합니다. NULL 비용은 0이 아니라 **사용할 수 없음**을 뜻합니다. 기준값이 없거나 0이면 변화율은 NULL입니다. 이벤트 수 0은 _이 구간에 일치하는 이벤트가 없음_을 뜻할 뿐 실행에 활동이 없었다는 증거가 아닙니다. 어느 한 세션에 일치하는 로그 행이 없다면 비교하기 전에 범위와 익스포터를 조사하십시오. 완료 상태, 경과 시간이나 활성 시간, 임계 경로, 인과적·통계적 결론은 이 이벤트에서 추론하지 않습니다.

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

실행 두 번으로 보여 주는 것은 차이일 뿐 통계적 유의성이나 인과 관계가 아닙니다. 도구 이벤트는 각 도구 스팬을 추적하지 않고 완료된 결과를 셉니다. 이 레시피는 중첩, 재시도, 겹치는 작업을 구분할 수 없습니다. 모델/도구의 추가와 제거를 빠짐없이 보려면 내부 조인으로 없는 그룹을 숨기지 말고 각 ID의 그룹화된 요청 및 결과 이벤트를 살펴보십시오.

## 세션 비용 순위

이 순위표는 비용 이상 탐지기가 **아닙니다**. 유한한 구간 안에 API 요청 이벤트가 하나 이상 있는 Claude Code 세션만 최대 50행까지 순위를 매깁니다. `priced_requests / requests`가 비율을 보여 주며, 보고된 비용이 없는 세션도 NULL 비용으로 표시되어 순서의 맨 뒤에 남습니다. 구간 경계를 넘는 세션의 금액은 일부만입니다. 서로 관련 없는 작업에 전체 중앙값이나 추론한 코호트를 적용하는 것은 적절하지 않으며, 불완전한 세션을 몰래 완료로 분류하지도 않습니다. 쿼리가 메모리나 결과 제한에 닿으면 구간을 좁히십시오.

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

## 실행하고 근거 확인하기

SQL 블록 중 하나를 `comparison.sql`이나 `leaderboard.sql`로 저장하고, 그 원문을 `/v1/query`로 POST하십시오(필요하면 쿼리 자격 증명과 함께).

```bash
curl -sS -D - -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' --data-binary @comparison.sql
```

MCP에서는 **같은 SQL 텍스트**를 `sigiro mcp`의 `run_query` 도구에 주십시오. 별도의 엔드포인트는 없습니다. 행을 해석하기 전에 `x-sigiro-truncated`와 쿼리 오류를 확인하십시오. 보고된 요청을 살펴보려면 _같은 리터럴 UTC 범위_, `body = 'claude_code.api_request'`, 선택한 `session.id`로 `sigiro_logs`를 쿼리하고, 근거로 `timestamp`, `log_attributes`, `trace_id`를 포함하십시오. 로그에는 `trace_id`가 없을 수 있으므로 트레이스 링크를 지어내지 마십시오. 실제 트레이스 ID가 반환되었을 때만 `/v1/traces/{trace_id}`를 여십시오. 이벤트 속성에는 신원과 민감한 내용이 들어갈 수 있으므로 쿼리 접근을 보호하십시오.

이 레시피는 에이전트를 가로지르는 정규 세션, 엔드 투 엔드 수명 주기/소요 시간, 위임된 작업, 모델/도구 집합 차이, 완전한 비용 귀속, 코호트 대비 비교를 제공하지 않습니다. 수집과 진단은 [에이전트 텔레메트리](/docs/how-to/agent-telemetry)와 [에이전트 최적화](/docs/how-to/debug-agent-performance)를 참조하십시오. 원천 이벤트 필드는 Claude Code의 [monitoring reference](https://code.claude.com/docs/en/monitoring-usage)에 정의되어 있습니다.
