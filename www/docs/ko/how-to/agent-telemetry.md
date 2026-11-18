---
title: 에이전트 자체 텔레메트리를 sigiro로 전송하기
description: >-
  Claude Code와 Codex는 모두 자체 세션에 대한 OpenTelemetry를 내보냅니다. 이를 sigiro로 보내면 토큰 수, 도구
  호출, 세션 지연 시간을 SQL로 다시 조회할 수 있습니다.
sidebar:
  order: 3
---
이 가이드에서는 서비스가 하는 일이 아니라 에이전트가 하는 일을 기록하는 방법을
설명합니다. Claude Code와 Codex는 모두 자체 세션에 대한 OpenTelemetry를
내보냅니다. 해당 텔레메트리를 sigiro로 보내면 세션이 조회 가능한 행이 됩니다.

이는 _사용자의 서비스_ 를 sigiro로 연결하는
[에이전트로 코드 계측하기](/docs/how-to/install-with-ai) 와는 다른 작업입니다.
둘 다 필요하다면 둘 다 수행하십시오.

## 시작하기 전에

sigiro 서버가 필요합니다. `sigiro serve` 명령으로 서버를 시작하거나,
[빠른 시작](/docs/tutorials/quickstart#1-run-it) 의 Docker 명령을 사용하십시오.

아래 설정에서 두 에이전트 모두 `4317` 포트에서 OTLP gRPC로 내보냅니다.
자체 호스팅 서버에는 키가 필요하지 않습니다.

호스팅 서비스에서는 대신 **HTTP** 기반 OTLP를 사용하십시오. 호스팅 운영자가
gRPC 포트를 항상 노출하지는 않으므로, 먼저 [호스팅 sigiro로 텔레메트리
전송하기](/docs/how-to/hosted-onboarding#2-point-your-opentelemetry-exporter-at-sigiro)
를 읽어 보십시오. 아래 각 절의 끝에 호스팅 설정이 나와 있습니다.

## Claude Code

Claude Code는 환경에서 텔레메트리 설정을 읽습니다. Claude Code를 시작하는
환경에 다음 변수를 설정하십시오. 셸 프로파일, `.envrc` 파일 또는 래퍼
스크립트를 사용하십시오:

```bash
export CLAUDE_CODE_ENABLE_TELEMETRY=1
export CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1
export OTEL_METRICS_EXPORTER=otlp
export OTEL_LOGS_EXPORTER=otlp
export OTEL_TRACES_EXPORTER=otlp
export OTEL_EXPORTER_OTLP_PROTOCOL=grpc
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
```

호스팅 서비스의 경우 키를 추가하는 것뿐만 아니라 프로토콜과 엔드포인트도
변경하십시오. `api.sigiro.com` 은 443 포트에서만 응답하므로, 해당 호스트에
`:4317` 또는 `:4318` 엔드포인트를 지정하면 연결에 실패하고 에이전트는 아무것도
보고하지 않습니다:

```bash
export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
export OTEL_EXPORTER_OTLP_ENDPOINT=https://api.sigiro.com
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer ${SIGIRO_API_KEY}"
```

키는 커밋되는 모든 파일에서 제외하십시오. 사용할 엔드포인트와 포트는 운영자가
알려주므로, 해당 호스트가 `api.sigiro.com` 이 아닌 경우
`https://<your-sigiro-host>` 를 사용하십시오.

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
endpoint = "https://api.sigiro.com/v1/traces"
protocol = "binary"
headers = { authorization = "Bearer ${SIGIRO_API_KEY}" }

[otel.metrics_exporter.otlp-http]
endpoint = "https://api.sigiro.com/v1/metrics"
protocol = "binary"
headers = { authorization = "Bearer ${SIGIRO_API_KEY}" }

[otel.exporter.otlp-http]
endpoint = "https://api.sigiro.com/v1/logs"
protocol = "binary"
headers = { authorization = "Bearer ${SIGIRO_API_KEY}" }
```

`otlp-http` 에는 `protocol` 이 필수이며, `binary` 또는 `json` 값을 받습니다.
Codex를 시작하는 환경에 `SIGIRO_API_KEY` 를 설정하십시오.

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

에이전트는 자신을 하나의 서비스로 보고합니다. 서비스가 나타나면 세션을
조회하십시오:

```bash
curl -X POST http://localhost:9999/v1/query \
  --data "SELECT service_name, span_name, count(*) AS n
          FROM sigiro_spans
          WHERE timestamp > now() - INTERVAL '1 hour'
          GROUP BY 1, 2 ORDER BY n DESC"
```

에이전트는 카운터와 게이지도 보고하며, 이는 `sigiro_spans` 가 아니라
`sigiro_metrics_*` 테이블에 저장됩니다. [테이블
소개](/docs/explanation/tables) 에서 어떤 계열이 무엇을 담고 있는지, 그리고 각
계열이 어떤 머신을 설명하는지 확인할 수 있습니다.

## 다음 단계

- [에이전트로 코드 계측하기](/docs/how-to/install-with-ai) — 사용자의 서비스도
  sigiro로 연결하기
- [CLI 레퍼런스](/docs/reference/cli) — 모든 하위 명령과 플래그
