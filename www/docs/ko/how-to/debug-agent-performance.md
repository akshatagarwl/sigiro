---
title: sigiro로 에이전트 디버깅 및 최적화하기
description: 코딩 에이전트 세션을 기록하고, 시간과 토큰을 어디에 썼는지 찾고, 그 근거를 더 작고 빠른 워크플로로 바꿉니다.
sidebar:
  order: 5
---

이 가이드는 에이전트 자체의 OpenTelemetry를 사용해 네 가지 질문에 답합니다.

1. 세션은 실제 경과 시간을 어디에 썼습니까?
2. 어떤 모델, 도구, 작업이 가장 많은 토큰이나 호출을 소비했습니까?
3. 어떤 실패, 재시도, 반복 읽기가 쓸모 있는 일을 하지 않았습니까?
4. 제안한 최적화가 비교 가능한 두 번째 실행을 개선했습니까?

느린 스팬 하나나 모델의 자기 보고를 근거로 최적화하지 마십시오. 범위가 정해진 세션 구간을 사용하고, 작업 요약 전체의 순위를 매기고, 모든 주장 뒤에 있는 SQL을 보존하십시오.

## 1. 격리된 로컬 sigiro 시작하기

최신 릴리스를 설치하고 서버를 시작합니다.

```bash
curl -fsSL https://sigiro.com/install | sh
export PATH="$HOME/.local/bin:$PATH"

export SIGIRO_DATA_DIR="$HOME/.local/share/sigiro-agent-debug"
unset SIGIRO_CATALOG_URL
unset SIGIRO_S3_BUCKET SIGIRO_S3_ENDPOINT SIGIRO_S3_REGION
unset SIGIRO_S3_KEY_ID SIGIRO_S3_SECRET
unset AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY AWS_SESSION_TOKEN

sigiro serve
```

`SIGIRO_CATALOG_URL`을 설정하지 않으면 sigiro는 텔레메트리를 로컬 Parquet 파일에 저장하고 그 메타데이터를 로컬 DuckDB 카탈로그에 보관합니다. PostgreSQL이나 S3는 사용하지 않습니다. 데이터 디렉터리를 명시하면 이 실험이 다른 로컬 텔레메트리와 분리됩니다.

다른 터미널에서 준비될 때까지 기다립니다.

```bash
sigiro healthcheck
curl http://127.0.0.1:9999/v1/services
```

그런 다음 Claude Code, Codex, Hermes Agent, Pi에 대해 [에이전트 자체 텔레메트리를 sigiro로 전송하기](/docs/how-to/agent-telemetry)를 따르십시오.

## 2. 쓸모 있는 테스트 세션 기록하기

완료 여부를 분명히 확인할 수 있는 작업을 고르십시오. 시작하기 전에 다음을 기록하십시오.

- 정확한 프롬프트
- 저장소 리비전과 작업 트리 상태
- 에이전트, 모델, 설정
- UTC 시작 및 종료 타임스탬프
- 캐시와 의존성이 워밍된 상태였는지 여부
- 기대하는 결과물과 검증 명령

중간에 개입하지 않고 작업을 한 번 실행하십시오. 프롬프트, 모델, 저장소 상태, 캐시 워밍 상태를 동시에 바꾸는 벤치마크로는 왜 빨라졌는지 알 수 없습니다.

실행 후 익스포터가 데이터를 생성했는지 확인합니다.

```bash
curl http://127.0.0.1:9999/v1/services
```

서비스 이름은 익스포터가 정합니다. 에이전트의 제품 이름과 같다고 가정하지 말고 여기서 확인하십시오.

## 3. 범위가 정해진 세션 구간 쿼리하기

실행을 포함하는 리터럴 UTC 타임스탬프로 `/v1/query`를 사용하십시오. 먼저 작업 수와 오류 스팬을 집계한 다음, 해당하는 트레이스 ID를 살펴보십시오. 결과는 원시 근거이며 진단이나 인과 판정이 아닙니다. 반환된 행을 완전하다고 보기 전에 `x-sigiro-truncated` 응답 헤더를 확인하십시오.

## 4. 경과 시간 비용의 순위 매기기

스팬을 `span_name`(가능하면 경로/주소 속성)으로 그룹화한 다음, 다음 패턴을 살펴보십시오.

- 모델 호출 하나가 p95와 전체 소요 시간을 대부분 차지한다
- 순차적인 도구 스팬 여러 개를 독립적으로 실행할 수 있었다
- 설정, 인증, 의존성 확인이 한 세션 안에서 반복된다
- 실패한 도구가 입력을 바꾸지 않고 재시도된다
- 같은 파일, URL, 상태 엔드포인트를 반복해서 가져온다
- 자식 에이전트가 부모가 이미 한 탐색을 반복한다
- 승인이나 사용자 대기가 실행 시간으로 계산된다
- 긴 부모 스팬은 자식의 시간을 포함할 뿐이므로 그것에 더해서는 안 된다

스팬 소요 시간은 포함 관계입니다. 부모 세션 스팬과 그 자식 모델 및 도구 스팬을 독립된 비용처럼 합산하지 마십시오. 부모로 엔드 투 엔드 지연 시간을 측정하고, 자식으로 그것을 설명하십시오.

사용자 정의 쿼리를 작성할 때는 Parquet 통계로 파일을 건너뛸 수 있도록 리터럴 UTC 범위를 사용하십시오.

```bash
curl -sS -D /tmp/sigiro-headers -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT span_name,
                 count(*) AS calls,
                 sum(duration) / 1000000.0 AS total_seconds,
                 max(duration) / 1000000.0 AS slowest_seconds
          FROM sigiro_spans
          WHERE service_name = '<service-name>'
            AND timestamp >= TIMESTAMP '2026-09-10 12:00:00'
            AND timestamp <  TIMESTAMP '2026-09-10 13:00:00'
          GROUP BY span_name
          ORDER BY total_seconds DESC"
```

`x-sigiro-truncated` 응답 헤더를 확인하십시오. `true`이면 완전성을 주장하기 전에 쿼리 범위를 좁히거나 더 집계하십시오. 쿼리 엔드포인트는 읽기 전용 `SELECT` 하나만 받으며 CTE는 받지 않습니다.

## 5. 근거를 변경으로 바꾸기

측정된 주요 비용을 해결하는 가장 작은 변경을 적용하십시오.

- **직렬화된 독립 도구:** 함께 요청하거나 독립적인 조사를 동시에 위임합니다.
- **반복되는 탐색:** 선행 조건을 한 번만 모으고, 각 자식이 다시 찾게 하는 대신 그 결과를 자식 에이전트에게 전달합니다.
- **너무 큰 도구 결과:** 원천에서 필터링하고, 필드를 덜 요청하고, 파일 범위를 좁히고, 로그 전체를 모델에 다시 넣지 않습니다.
- **과도한 모델 컨텍스트:** 중복된 지시와 오래된 이력을 제거하고, 현재 결정에 필요한 파일과 레코드만 가져옵니다.
- **반복되는 실패:** 재시도하기 전에 누락된 경로, 자격 증명, 의존성, 잘못된 인수를 고칩니다. 입력이 같은 재시도는 복구가 아닙니다.
- **너무 많은 모델 호출:** 서로 의존하는 작은 질문은 범위가 정해진 요청 하나로 묶고, 독립적인 작업은 병렬로 유지합니다.
- **한 모델에 집중된 비용:** 기계적인 탐색에는 더 저렴한 모델을 쓰고, 고성능 모델은 그것이 필요한 결정에 남겨 둡니다.
- **긴 승인 대기:** 실행 속도 개선을 주장하기 전에 사람의 대기 시간과 계산 시간을 분리합니다.

토큰 수만으로 인과 관계를 추론하지 마십시오. 토큰이 많다는 것은 유용한 추론일 수도, 반복된 컨텍스트일 수도 있습니다. 관련 스팬, 오류, 종료 이유, 도구 순서를 읽으십시오.

## 6. 최적화 검증하기

같은 저장소 상태와 비교 가능한 캐시 조건에서 같은 프롬프트를 다시 실행하십시오. 같은 길이의 UTC 구간을 비교하고 다음을 보고하십시오.

- 엔드 투 엔드 세션 소요 시간
- 모델 호출 수와 p95 지연 시간
- 기록된 경우 입력, 출력, 캐시, 추론 토큰
- 도구 호출 수와 자식의 누적 소요 시간
- 실패한 작업과 재시도한 작업의 수
- 요청한 결과물이 같은 검증 명령을 통과했는지 여부

작업에 실패한 더 빠른 실행은 최적화가 아닙니다. 비교에 사용한 범위가 정해진 SQL과 쿼리 결과를 보존하십시오.

## 분석 에이전트에게 직접 접근 권한 주기

sigiro는 두 가지 기계용 인터페이스를 제공합니다.

- HTTP API용 `GET http://127.0.0.1:9999/openapi.json`
- stdio MCP 도구용 `sigiro mcp`: `list_services`와 `run_query`

Hermes Agent의 경우 `~/.hermes/config.yaml`에 다음을 추가하고, Hermes 환경에 Python `mcp` 패키지가 설치되어 있는지 확인한 다음 Hermes를 다시 시작하십시오.

```yaml
mcp_servers:
  sigiro:
    command: sigiro
    args: [mcp]
```

MCP를 지원하는 다른 에이전트도 각자의 MCP 설정으로 같은 `sigiro mcp` 명령을 등록할 수 있습니다. 에이전트 서비스의 `PATH`가 대화형 셸보다 좁다면 `sigiro`의 절대 경로를 사용하십시오.

## 분석 에이전트용 프롬프트

### 세션 병목 찾기

```text
Use Sigiro to analyze service <service-name> from <UTC start> to <UTC end>.
Use list_services, then run_query with literal UTC bounds to rank where the
session spent wall-clock time by operation and model. Distinguish inclusive
parent duration from child duration; do not double-count nested spans. Cite
trace IDs or SQL results for every conclusion. Check truncation before
claiming completeness. End with the three smallest changes most likely to reduce
end-to-end latency without weakening the task's verification.
```

### 낭비된 작업 찾기

```text
Audit service <service-name> from <UTC start> to <UTC end> for work that did not
advance the task: repeated file or URL reads, unchanged retries, failed setup or
authentication calls, duplicate parent/child discovery, oversized tool results,
and serial calls that were independent. Query bounded raw telemetry, then inspect
complete rows for suspicious operations. Report counts, duration,
and trace IDs. Separate evidence from inference.
```

### 모델과 토큰 비용 분석하기

```text
Analyze GenAI telemetry for service <service-name> from <UTC start> to <UTC end>.
Break down calls, p95 latency, errors, finish reasons, and available input,
output, cache, and reasoning tokens by provider and model. Identify where token
volume is concentrated and correlate it with the surrounding operation or tool
sequence. Do not recommend a cheaper model unless the evidence shows which calls
are mechanical enough to move safely.
```

### 이전과 이후 비교하기

```text
Compare service <service-name> in baseline window <UTC start/end> with optimized
window <UTC start/end>. Use equal query logic and literal TIMESTAMP bounds. Compare
end-to-end duration, model calls and p95, token categories, tool calls, failures,
and retries. Verify both runs completed the same acceptance check. Cite SQL and
rows for every delta, state missing telemetry plainly, and reject improvements
that only moved time into an unmeasured child process.
```

### 설정과 전달 작업 분리하기

```text
Analyze this agent session as two phases: prerequisite/setup work and delivery
work. Derive the boundary from span names and timestamps rather than assuming it.
Report duration, failures, retries, and model/tool calls in each phase. Identify
which setup checks can be cached or performed once, and which are required for
correctness. Include trace IDs and runnable drill-down SQL.
```
