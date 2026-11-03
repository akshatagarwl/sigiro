---
title: 에이전트 자체 텔레메트리를 sigiro로 전송하기
description: Claude Code, Codex, Hermes Agent, Pi가 sigiro로 보내도록 설정해 세션, 모델 지연 시간, 토큰, 도구, 오류, 재시도를 살펴봅니다.
sidebar:
  order: 4
---

이 가이드는 코딩 에이전트가 편집하는 애플리케이션이 아니라 코딩 에이전트 자체가 하는 일을 기록합니다. 그 결과로 생긴 스팬, 로그, 메트릭으로 느린 모델 호출, 반복되는 도구, 실패한 선행 조건, 과도한 컨텍스트, 직렬화된 작업을 찾을 수 있습니다.

이는 *사용자의 애플리케이션*을 sigiro로 향하게 하는 [에이전트로 코드 계측하기](/docs/how-to/install-with-ai)와는 다릅니다. 두 관점이 모두 필요하면 둘 다 사용하십시오.

## sigiro 스킬 설치하기

에이전트를 시작하기 전에 공개 스킬을 설치하십시오.

```bash
npx skills add Arnav12114/skills
```

[`debug-with-sigiro` 스킬](https://github.com/Arnav12114/skills)은 에이전트에게 sigiro에서 텔레메트리를 살펴보는 방법을 알려 줍니다. 이 가이드의 텔레메트리 설정은 바꾸지 않습니다.

## 시작하기 전에

sigiro를 시작하고 API를 확인하십시오.

```bash
sigiro serve
curl http://127.0.0.1:9999/health
```

로컬 수신기는 `4318`에서 OTLP/HTTP를, `4317`에서 OTLP/gRPC를 받습니다. 요청 인증이 없으므로 신뢰할 수 없는 네트워크에 노출하지 마십시오. 호스팅 서비스를 사용하려면 새 계정은 `sigiro signup --name "Your Name" --email you@example.com`을(기존 계정은 `sigiro auth login`을) 실행하고, 대신 [호스티드 sigiro로 텔레메트리 전송](/docs/how-to/hosted-onboarding)을 따르십시오.

텔레메트리에는 프롬프트, 응답, 파일 내용, 명령, 사용자 식별자, 환경 세부 정보가 들어갈 수 있습니다. 아래 예시는 보수적인 수집 설정을 고릅니다. 더 많은 내용을 켜기 전에 모든 익스포터를 검토하십시오.

## Claude Code

Claude Code에는 OpenTelemetry 내보내기가 내장되어 있습니다. 다음 키를 `~/.claude/settings.json`의 `env` 객체에 추가하십시오. 그러면 IDE가 시작하는 세션을 포함해 모든 세션이 텔레메트리를 보냅니다.

```json
{
  "env": {
    "CLAUDE_CODE_ENABLE_TELEMETRY": "1",
    "CLAUDE_CODE_ENHANCED_TELEMETRY_BETA": "1",
    "OTEL_TRACES_EXPORTER": "otlp",
    "OTEL_METRICS_EXPORTER": "otlp",
    "OTEL_LOGS_EXPORTER": "otlp",
    "OTEL_EXPORTER_OTLP_PROTOCOL": "http/protobuf",
    "OTEL_EXPORTER_OTLP_ENDPOINT": "http://127.0.0.1:4318",
    "OTEL_METRIC_EXPORT_INTERVAL": "10000"
  }
}
```

파일에 이미 `env` 객체가 있다면 키를 그 안에 병합하십시오. 같은 변수를 `claude`를 시작하는 셸에서 `export` 문으로 써도 됩니다.

Claude Code의 트레이싱은 베타입니다. `CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1`과 `OTEL_TRACES_EXPORTER=otlp`로 켭니다. 트레이스가 없으면 진단의 `gen_ai`와 `operation_summaries` 섹션은 비어 있지만, 로그나 메트릭을 보낸 서비스는 여전히 `/v1/services`에 나옵니다. 사용 중인 버전은 [Claude Code monitoring](https://code.claude.com/docs/en/monitoring-usage)을 참조하십시오.

새 Claude Code 세션을 시작하고 프롬프트 하나를 실행한 다음, 10초 기다렸다가 아래 확인 단계를 진행하십시오.

### Claude Code가 기본으로 보내는 것

모든 메트릭, 로그, 스팬에는 `user.email`, `user.account_uuid`, `user.account_id`, `user.id`, `organization.id`, `session.id`, `terminal.type`이 있습니다. 공유 서버나 호스팅 서버로 텔레메트리를 보내기 전에 이 ID 속성들을 고려하십시오.

다음 섹션의 설정이 없으면 텔레메트리는 각 도구의 이름은 담지만 인수는 담지 않습니다.

- **도구 호출:** 도구 이름, 성공 여부, 소요 시간, 입력과 결과의 바이트 크기, 권한 결정.
- **Bash 명령:** 고정된 목록에 있는 첫 프로그램만(`bash_argv0`, 예: `git` 또는 `other`)과 그 범주(`bash_command_class`).
- **도구 실패:** `ShellError`나 `TelemetrySafeError` 같은 범주만.
- **프롬프트:** `<REDACTED>`와 프롬프트 길이.
- **모델 요청:** `claude_code.api_request` 이벤트의 모델, 토큰, 첫 토큰까지의 시간, `cost_usd`.

### 더 자세히 수집하기

아래 각 설정은 명령, 파일 내용, 대화 텍스트 같은 더 많은 내용을 내보냅니다. 노출을 감수할 수 있는 설정만 켜고 새 세션을 시작하십시오.

| 설정                                   | 추가되는 것                                                                                                                                                           |
| -------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `OTEL_LOG_TOOL_DETAILS=1`              | 도구 스팬의 `full_command`, `file_path`, `skill_name`, `subagent_type`. 도구 이벤트의 `tool_input`과 `tool_parameters`. 오류 전문. MCP 서버 이름과 연결 오류.     |
| `OTEL_LOG_TOOL_CONTENT=1`              | 각 도구의 입력과 출력을 담은 `tool.output` 스팬 이벤트. 트레이스가 필요합니다.                                                                                       |
| `OTEL_LOG_USER_PROMPTS=1`              | `claude_code.user_prompt` 이벤트와 `claude_code.interaction` 스팬의 프롬프트 텍스트.                                                                                  |
| `OTEL_LOG_ASSISTANT_RESPONSES=1`       | `claude_code.assistant_response` 이벤트의 응답 텍스트. 설정하지 않으면 `OTEL_LOG_USER_PROMPTS`를 따릅니다. 응답을 가린 채로 두려면 `0`으로 설정하십시오.          |
| `OTEL_LOG_RAW_API_BODIES=1`            | 전체 API 요청과 응답 JSON을 `claude_code.api_request_body`와 `claude_code.api_response_body` 이벤트로 보냅니다. 요청 본문에는 대화 이력이 포함됩니다.            |
| `OTEL_METRICS_INCLUDE_VERSION=true`    | 메트릭과 이벤트의 `app.version`.                                                                                                                                      |
| `OTEL_METRICS_INCLUDE_ENTRYPOINT=true` | 메트릭과 이벤트의 `app.entrypoint`.                                                                                                                                   |

실패한 도구를 보면 차이를 알 수 있습니다. 기본 설정에서는 실패한 Read가 `TelemetrySafeError`를, 실패한 Bash 명령이 `ShellError`를 보고합니다. `OTEL_LOG_TOOL_DETAILS=1`이면 같은 실패가 실패한 명령이나 경로와 함께 `File does not exist. Note: your current working directory is …`와 `Shell command failed`로 보고됩니다.

Claude Code는 내용 속성을 `CLAUDE_CODE_OTEL_CONTENT_MAX_LENGTH`(기본 61,440자)에서 자릅니다. 긴 대화의 요청 본문은 그 길이에서 잘린 채로 도착합니다. sigiro는 그 크기의 값을 저장합니다. 더 큰 제한은 테스트하지 않았습니다.

### 상세 베타 트레이싱

상세 베타 트레이싱은 각 단계에서 모델이 받고 돌려준 내용도 기록합니다. 같은 `env` 객체에 다음 키를 추가하십시오.

```json
"ENABLE_BETA_TRACING_DETAILED": "1",
"BETA_TRACING_ENDPOINT": "http://127.0.0.1:4318"
```

이 키가 있으면 Claude Code는 로그와 트레이스를 `OTEL_EXPORTER_OTLP_ENDPOINT` 대신 `BETA_TRACING_ENDPOINT`로 보냅니다. 메트릭은 여전히 `OTEL_EXPORTER_OTLP_ENDPOINT`로 갑니다. 둘 다 같은 sigiro 기본 URL을 사용하십시오.

상세 베타 트레이싱이 추가하는 것:

- 훅 실행마다 `claude_code.hook` 스팬(`OTEL_LOG_TOOL_DETAILS=1`이면 훅 설정 포함)
- 상호작용, 모델 요청, 도구 스팬의 `new_context`: 그 단계에서 Claude Code가 모델 컨텍스트에 추가한 텍스트
- 모델 요청 스팬의 `response.model_output`, `query_source`, `system_prompt_preview`, `system_prompt_length`, `system_prompt_hash`, `system_reminders`, `tools`
- `claude_code.system_prompt` 로그 이벤트

`system_reminders`는 모델 요청마다 수십 킬로바이트가 될 수 있습니다. Claude Code는 이 속성들을 안정된 스팬 스키마에 포함하지 않으므로 이름이 바뀔 수 있습니다. 대화형 세션에서는 조직이 Anthropic의 허용 목록에 있을 때만 받을 수 있습니다. `claude -p`와 Agent SDK 세션에는 허용 목록이 필요하지 않습니다. Claude Code는 프로젝트 및 로컬 설정 파일에 있는 이 두 키를 무시합니다.

### 하위 프로세스의 트레이스

Claude Code는 각 Bash 명령에 `TRACEPARENT` 변수를 줍니다. Claude Code Bash 도구가 시작한 `claude -p` 실행은 부모 세션의 트레이스에 합류합니다. 그 `claude_code.interaction` 스팬에는 `parent.source: env`가 있으며, `/v1/traces/{trace_id}`는 두 세션의 스팬을 모두 반환합니다. `session.id`로 필터링해 둘을 구분하십시오.

### Claude Code 텔레메트리 읽기

```text
claude_code.interaction                  one user prompt
├── claude_code.llm_request              one model request
├── claude_code.tool                     permission wait and execution
│   ├── claude_code.tool.blocked_on_user permission wait
│   │   └── claude_code.llm_request      auto mode permission check
│   └── claude_code.tool.execution       execution
└── claude_code.hook                     detailed beta tracing only
```

- `claude_code.tool`의 소요 시간에는 권한 대기가 포함됩니다. 실행 시간에는 `claude_code.tool.execution`을, 대기 시간에는 `claude_code.tool.blocked_on_user`를 사용하십시오.
- auto 모드에서 Claude Code는 일부 도구 호출을 별도의 모델 요청으로 확인합니다. 그 요청은 `claude_code.tool.blocked_on_user` 아래의 `claude_code.llm_request` 스팬이며 `query_source_safe = 'auto_mode'`가 붙습니다. Claude Code 2.1.273에서는 확인마다 약 1.4초가 더해졌습니다. 이 스팬에는 토큰 수가 있지만, Claude Code는 이에 대해 `claude_code.api_request` 이벤트도 `claude_code.cost.usage` 데이터 포인트도 보내지 않습니다. Claude Code 비용 합계에는 이 확인이 포함되지 않습니다.
- 실패한 도구는 스팬 상태와 `claude_code.tool_result` 이벤트의 `success = false`를 설정합니다. Claude Code는 이에 대해 ERROR 심각도 로그를 보내지 않습니다.
- `claude_code.api_request` 이벤트와 `claude_code.cost.usage` 메트릭은 같은 총비용을 보고합니다.

이 쿼리들의 타임스탬프를 세션을 감싸는 리터럴 UTC 범위로 바꾸십시오. 일별·모델별 비용:

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT strftime(timestamp, '%Y-%m-%d') AS usage_date,
                 json_extract_string(log_attributes, 'model') AS model,
                 count(*) AS requests,
                 round(sum(CAST(json_extract_string(log_attributes, 'cost_usd') AS DOUBLE)), 2) AS cost_usd
          FROM sigiro_logs
          WHERE service_name = 'claude-code'
            AND body = 'claude_code.api_request'
            AND timestamp >= TIMESTAMP '2026-09-10 00:00:00'
            AND timestamp <  TIMESTAMP '2026-09-11 00:00:00'
          GROUP BY usage_date, model
          ORDER BY usage_date, cost_usd DESC"
```

도구별 호출 수, 실패 수, p95 소요 시간:

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT json_extract_string(log_attributes, 'tool_name') AS tool,
                 count(*) AS calls,
                 count(*) FILTER (WHERE json_extract_string(log_attributes, 'success') = 'false') AS failures,
                 quantile_cont(CAST(json_extract_string(log_attributes, 'duration_ms') AS DOUBLE), 0.95) AS p95_ms
          FROM sigiro_logs
          WHERE service_name = 'claude-code'
            AND body = 'claude_code.tool_result'
            AND timestamp >= TIMESTAMP '2026-09-10 00:00:00'
            AND timestamp <  TIMESTAMP '2026-09-11 00:00:00'
          GROUP BY tool
          ORDER BY calls DESC"
```

권한 대기와 실행 시간 비교:

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT span_name,
                 count(*) AS spans,
                 round(sum(duration) / 60000000.0, 1) AS total_minutes,
                 median(duration) / 1000 AS p50_ms
          FROM sigiro_spans
          WHERE service_name = 'claude-code'
            AND span_name IN ('claude_code.tool.blocked_on_user', 'claude_code.tool.execution')
            AND timestamp >= TIMESTAMP '2026-09-10 00:00:00'
            AND timestamp <  TIMESTAMP '2026-09-11 00:00:00'
          GROUP BY span_name"
```

Claude Code 비용 합계에 들어가지 않는 auto 모드 권한 확인:

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT json_extract_string(span_attributes, 'model') AS model,
                 count(*) AS checks,
                 round(sum(duration) / 60000000.0, 1) AS total_minutes,
                 sum(CAST(json_extract_string(span_attributes, 'cache_read_tokens') AS BIGINT)) AS cache_read_tokens,
                 sum(CAST(json_extract_string(span_attributes, 'output_tokens') AS BIGINT)) AS output_tokens
          FROM sigiro_spans
          WHERE service_name = 'claude-code'
            AND span_name = 'claude_code.llm_request'
            AND json_extract_string(span_attributes, 'query_source_safe') = 'auto_mode'
            AND timestamp >= TIMESTAMP '2026-09-10 00:00:00'
            AND timestamp <  TIMESTAMP '2026-09-11 00:00:00'
          GROUP BY model"
```

## Codex

Codex는 `~/.codex/config.toml`에서 텔레메트리 설정을 읽습니다.

```toml
[otel]
environment = "dev"
log_user_prompt = false

[otel.trace_exporter.otlp-grpc]
endpoint = "http://127.0.0.1:4317"

[otel.metrics_exporter.otlp-grpc]
endpoint = "http://127.0.0.1:4317"

[otel.exporter.otlp-grpc]
endpoint = "http://127.0.0.1:4317"
```

`exporter`는 로그 익스포터이며 `trace_exporter`나 `metrics_exporter`를 대신하지 않습니다. Codex는 자신을 `codex_exec`로 보고하므로, 확인할 때 그 서비스 이름을 찾으십시오.

## Hermes Agent

Hermes는 커뮤니티 [`briancaffey/hermes-otel`](https://github.com/briancaffey/hermes-otel) 플러그인을 사용합니다.

```bash
hermes plugins install briancaffey/hermes-otel/hermes_otel
```

플러그인 요구 패키지를 Hermes를 실행하는 같은 Python 환경에 설치하십시오. 가상 환경이 `~/git/hermes-agent/venv`인 소스 체크아웃의 경우:

```bash
~/git/hermes-agent/venv/bin/pip install \
  -r ~/.hermes/plugins/hermes_otel/requirements.txt
```

설치 환경에 맞는 `pip` 경로를 사용하십시오. 관련 없는 시스템 Python에 이 패키지를 설치해도 Hermes에서는 보이지 않습니다.

`~/.hermes/hermes_otel.yaml`을 만드십시오.

```yaml
backends:
  - type: otlp
    name: sigiro
    endpoint: http://127.0.0.1:4318/v1/traces
    metrics: true
    logs: true

capture_logs: true
capture_previews: false
capture_sender_id: false
capture_conversation_history: false

# Optional host-level evidence for local performance work.
host_metrics: true
host_metrics_gpu: auto
host_metrics_interval_ms: 1000
```

플러그인을 설치하거나 바꾼 뒤에는 Hermes를 다시 시작하십시오. 범용 OTLP 백엔드는 트레이스 엔드포인트에서 `/v1/metrics`와 `/v1/logs`를 도출합니다. 세션, 모델, API, 도구, 스킬, 승인, 하위 에이전트의 중첩 스팬과 함께 토큰, 소요 시간, 비용, 재시도, 리소스 메트릭을 내보냅니다.

플러그인의 `capture_previews` 기본값은 `true`입니다. 위의 명시적 `false`는 프롬프트, 응답, 도구 인수, 도구 결과 미리 보기가 내보내지지 않게 합니다. `capture_logs: true`는 여전히 가져온 라이브러리의 Python 레코드를 내보낼 수 있습니다. 로거 범위를 더 좁혀야 한다면 플러그인의 `log_attach_logger` 설정을 사용하십시오.

## Pi 코딩 에이전트

Pi의 공식 `@earendil-works/pi-telemetry` 패키지는 텔레메트리 타입을 정의하지만 현재 OTLP 익스포터는 제공하지 않습니다. 따라서 다음 설정은 커뮤니티가 관리하는 [`@devkade/pi-opentelemetry`](https://github.com/devkade/pi-opentelemetry) 확장을 사용합니다. 도입하기 전에 검토하십시오.

```bash
pi install npm:@devkade/pi-opentelemetry

export PI_OTEL_ENABLE=1
export OTEL_SERVICE_NAME=pi-coding-agent

export OTEL_TRACES_EXPORTER=otlp
export OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=http://127.0.0.1:4318/v1/traces

export OTEL_METRICS_EXPORTER=otlp
export OTEL_EXPORTER_OTLP_METRICS_ENDPOINT=http://127.0.0.1:4318/v1/metrics
export OTEL_METRIC_EXPORT_INTERVAL=10000

export PI_OTEL_PRIVACY_PROFILE=strict
export PI_OTEL_PAYLOAD_MAX_BYTES=32768

pi
```

Pi 안에서 `/otel-status`는 익스포터 상태를 보고하고 `/otel-open-trace`는 현재 트레이스를 엽니다. 이 확장의 문서는 트레이스와 메트릭을 다루며 로그 내보내기는 다루지 않습니다. `strict` 개인정보 프로필은 기본 `detailed-with-redaction` 프로필보다 덜 노출합니다.

## 텔레메트리 도착 확인하기

설정한 에이전트에서 실제 프롬프트 하나를 실행한 다음 서비스 이름을 확인하십시오.

```bash
curl http://127.0.0.1:9999/v1/services
```

에이전트 항목을 찾아 `signals.spans`, `signals.logs`, `signals.metrics`를 살펴보십시오. 스팬은 0이지만 로그나 메트릭이 0이 아닌 서비스는 텔레메트리를 보내고 있는 것이며, 설치 실패가 아닙니다. 스팬 기반 진단(`gen_ai`와 `operation_summaries`)은 `signals.spans`가 0이 아닐 때만 기대하십시오. 트레이스를 켰는데 스팬이 0이라면 에이전트의 트레이스 익스포터 설정을 확인하십시오. 내보내지는 어휘를 추측하지 마십시오. 개수가 0이 아닌 신호 테이블만 쿼리하십시오. 두 타임스탬프를 테스트 세션을 감싸는 리터럴 UTC 범위로 바꾸십시오. 스팬의 경우:

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT service_name, span_name, count(*) AS occurrences
          FROM sigiro_spans
          WHERE timestamp >= TIMESTAMP '2026-09-10 12:00:00'
            AND timestamp <  TIMESTAMP '2026-09-10 13:00:00'
          GROUP BY service_name, span_name
          ORDER BY occurrences DESC"
```

`signals.logs`가 0이 아니면 로그를 살펴보십시오.

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT service_name, count(*) AS records
          FROM sigiro_logs
          WHERE timestamp >= TIMESTAMP '2026-09-10 12:00:00'
            AND timestamp <  TIMESTAMP '2026-09-10 13:00:00'
          GROUP BY service_name ORDER BY records DESC"
```

`signals.metrics`가 0이 아니면 OpenTelemetry 테이블 유형 전반에서 메트릭 이름을 찾으십시오.

```bash
for table in gauge sum histogram exp_histogram; do
  curl -sS -X POST http://127.0.0.1:9999/v1/query \
    -H 'content-type: text/plain' \
    --data "SELECT metric_name, count(*) AS points
            FROM sigiro_metrics_${table}
            WHERE timestamp >= TIMESTAMP '2026-09-10 12:00:00'
              AND timestamp <  TIMESTAMP '2026-09-10 13:00:00'
            GROUP BY metric_name ORDER BY points DESC"
done
```

리터럴 `TIMESTAMP` 범위를 쓰면 sigiro가 Parquet 파일을 걸러 낼 수 있습니다. `now()`로 만든 조건은 모든 파일을 읽으며, 로컬 이력이 늘어날수록 점점 느려집니다.

다음으로 [sigiro로 에이전트 디버깅 및 최적화하기](/docs/how-to/debug-agent-performance)를 보십시오.
