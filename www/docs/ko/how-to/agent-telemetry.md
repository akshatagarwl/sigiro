---
title: 에이전트 자체 텔레메트리를 sigiro로 전송하기
description: >-
  Claude Code와 Codex는 자체 세션에 대한 OpenTelemetry를 내보냅니다. sigiro로 보내면 토큰 수와 도구 호출,
  세션 지연 시간을 SQL로 조회할 수 있습니다.
sidebar:
  order: 4
---

이 가이드에서는 서비스가 하는 일이 아니라 에이전트가 하는 일을 기록하는 방법을
설명합니다. Claude Code와 Codex는 모두 자체 세션에 대한 OpenTelemetry를
내보냅니다. 해당 텔레메트리를 sigiro로 보내면 세션이 조회 가능한 행이 됩니다.

이는 _사용자의 서비스_ 를 sigiro로 연결하는
[에이전트로 코드 계측하기](/docs/how-to/install-with-ai) 와는 다른 작업입니다.
둘 다 필요하다면 둘 다 수행하십시오.

## 시작하기 전에

sigiro 서버가 필요합니다. `sigiro serve` 명령으로 서버를 시작하거나,
[빠른 시작](/docs/tutorials/quickstart#1-실행하기) 의 Docker 명령을 사용하십시오.

아래 설정에서 Claude Code는 `4318` 포트로 OTLP HTTP를, Codex는 `4317` 포트로 OTLP gRPC를 사용합니다.
자체 호스팅 서버에는 키가 필요하지 않습니다.

호스팅 서비스의 신규 사용자는 `sigiro signup --name "Your Name" --email you@example.com`을, 기존 계정은 `sigiro auth login`을 실행하십시오.

호스팅 서비스에서는 대신 **HTTP** 기반 OTLP를 사용하십시오. 호스팅 운영자가
gRPC 포트를 항상 노출하지는 않으므로, 먼저 [호스팅 sigiro로 텔레메트리
전송하기](/docs/how-to/hosted-onboarding#2-opentelemetry-익스포터를-sigiro로-지정하기)
를 읽어 보십시오. 아래 각 절의 끝에 호스팅 설정이 나와 있습니다.

## Claude Code

`~/.claude/settings.json`의 `env`에 다음 키를 추가하십시오. 기존 `env`가 있다면 키를 병합하십시오. IDE에서 시작한 세션에도 적용됩니다. 셸에서 `claude`를 실행한다면 동일한 변수를 `export`해도 됩니다:

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

Claude Code의 트레이싱은 베타 기능입니다. `CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1`과 `OTEL_TRACES_EXPORTER=otlp`으로 활성화합니다. 자세한 내용은 [공식 모니터링 가이드](https://code.claude.com/docs/en/monitoring-usage)를 참고하십시오. 새 세션에서 프롬프트를 한 번 실행한 뒤 아래 절차로 확인하십시오.

호스팅 서비스의 경우 `env`의 `OTEL_EXPORTER_OTLP_ENDPOINT`를 `https://sigiro.com`으로 변경하고 인증 헤더를 설정하십시오. `sigiro.com`은 443 포트에서만 응답하므로 `:4317`이나 `:4318`을 지정하지 마십시오. 셸에서 시작한다면 위 설정의 모든 변수를 `export`하고 다음 값을 사용합니다:

```bash
export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
export OTEL_EXPORTER_OTLP_ENDPOINT=https://sigiro.com
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer ${SIGIRO_ACCESS_TOKEN}"
```

키는 커밋되는 모든 파일에서 제외하십시오. 사용할 엔드포인트와 포트는 운영자가
알려주므로, 해당 호스트가 `sigiro.com` 이 아닌 경우
`https://<your-sigiro-host>` 를 사용하십시오.

기본적으로 실패한 도구는 `TelemetrySafeError` 또는 `ShellError`와 같은 일반적인
상태만 보고할 수 있습니다. 디버깅 중 실패 원인을 확인하려면 도구 세부 정보를
명시적으로 활성화하십시오:

```bash
export OTEL_LOG_TOOL_DETAILS=1
```

이 설정을 활성화하면 명령, 파일 경로, 오류 텍스트가 전송될 수 있습니다. 추가적인
개인정보 노출을 허용할 수 있을 때만 활성화하십시오.

## Codex

Codex는 환경이 아니라 자체 설정 파일에서 텔레메트리 설정을 읽습니다.
`~/.codex/config.toml` 파일을 편집하십시오:

```toml
[otel]
environment = "dev"
log_user_prompt = false

[otel.trace_exporter.otlp-grpc]
endpoint = "http://localhost:4317"

[otel.metrics_exporter.otlp-grpc]
endpoint = "http://localhost:4317"

[otel.exporter.otlp-grpc]
endpoint = "http://localhost:4317"
```

Codex는 시그널별로 익스포터를 하나씩 지정하며, `exporter` 는 **로그** 익스포터
전용입니다. `trace_exporter` 를 설정하지 않으면 스팬을 얻을 수 없고,
`metrics_exporter` 를 설정하지 않으면 메트릭을 얻을 수 없습니다. 두 실수 모두
아무런 오류 없이 조용히 발생합니다.

호스팅 서비스의 경우 HTTP 익스포터를 사용하고 각 엔드포인트에 **전체 시그널
경로**를 지정하십시오. Codex는 작성된 URL 그대로 전송하며 아무것도 덧붙이지
않으므로, 기본 URL을 지정하면 잘못된 경로로 도착하여 sigiro가 이를 거부합니다:

```toml
[otel]
environment = "dev"
log_user_prompt = false

[otel.trace_exporter.otlp-http]
endpoint = "https://sigiro.com/v1/traces"
protocol = "binary"
headers = { authorization = "Bearer ${SIGIRO_ACCESS_TOKEN}" }

[otel.metrics_exporter.otlp-http]
endpoint = "https://sigiro.com/v1/metrics"
protocol = "binary"
headers = { authorization = "Bearer ${SIGIRO_ACCESS_TOKEN}" }

[otel.exporter.otlp-http]
endpoint = "https://sigiro.com/v1/logs"
protocol = "binary"
headers = { authorization = "Bearer ${SIGIRO_ACCESS_TOKEN}" }
```

`otlp-http` 에는 `protocol` 이 필수이며, `binary` 또는 `json` 값을 받습니다.
Codex를 시작하는 환경에 `SIGIRO_ACCESS_TOKEN` 를 설정하십시오.

Codex는 자신을 `codex` 가 아니라 `codex_exec` 로 보고하므로, 아래 확인 단계에서
해당 이름을 찾으십시오.

## 두 경우 모두에 적용되는 한 가지 제약

이 설정은 에이전트 프로세스에만 적용됩니다. 에이전트가 실행하는 명령에는
적용되지 않습니다. 에이전트가 셸 도구를 통해 시작한 애플리케이션이 이러한
변수를 항상 상속하지는 않으므로, 계측된 서비스에는 여전히 자체 내보내기 설정이
필요합니다. 이를 위해서는
[에이전트로 코드 계측하기](/docs/how-to/install-with-ai) 를 사용하십시오.

## 정상 동작 확인

에이전트를 시작하고 프롬프트를 하나 실행한 다음, sigiro가 어떤 서비스를
수신했는지 확인하십시오:

```bash
curl http://localhost:9999/v1/services
```

서비스의 `signals.spans`, `signals.logs`, `signals.metrics`를 확인하십시오. 스팬이 0이어도 로그나 메트릭이 있다면 텔레메트리가 도착한 것입니다. 진단의 `gen_ai`와 `operation_summaries`에는 스팬이 필요합니다. 트레이싱을 활성화했는데 스팬이 0이라면 익스포터 설정을 확인하십시오. 개수가 0이 아닌 신호의 테이블만 조회하십시오. 스팬이 있을 때:

```bash
curl -X POST http://localhost:9999/v1/query \
  --data "SELECT service_name, span_name, count(*) AS n
          FROM sigiro_spans
          WHERE timestamp > now() - INTERVAL '1 hour'
          GROUP BY 1, 2 ORDER BY n DESC"
```

로그가 있다면 `sigiro_logs`를 확인하십시오:

```bash
curl -X POST http://localhost:9999/v1/query \
  --data "SELECT service_name, count(*) AS records
          FROM sigiro_logs
          WHERE timestamp > now() - INTERVAL '1 hour'
          GROUP BY service_name ORDER BY records DESC"
```

메트릭이 있다면 `sigiro_metrics_gauge`, `sigiro_metrics_sum`, `sigiro_metrics_histogram`, `sigiro_metrics_exp_histogram`을 확인하십시오. 각 테이블의 내용은 [테이블 소개](/docs/explanation/tables)를 참조하십시오.

## 다음 단계

- [에이전트로 코드 계측하기](/docs/how-to/install-with-ai) — 사용자의 서비스도
  sigiro로 연결하기
- [CLI 레퍼런스](/docs/reference/cli) — 모든 하위 명령과 플래그
