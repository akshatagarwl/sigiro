---
title: 에이전트로 코드 계측하기
description: >-
  에이전트에게 프롬프트 하나를 전달하면, 서비스에 OpenTelemetry 자동 계측을 추가하고 sigiro를 가리킵니다. Claude
  Code, OpenCode, Codex에서 동작합니다.
sidebar:
  order: 1
---
이 가이드에서는 계측이 전혀 없는 서비스에 OpenTelemetry를 추가하는 방법을 설명하며, 편집 작업은 에이전트가 수행합니다. 아래 프롬프트를 에이전트에 붙여넣으십시오. 에이전트가 프레임워크를 감지하고, 공식 자동 계측을 설치한 뒤, 익스포터가 sigiro를 가리키도록 설정합니다.

이 방법은 Claude Code, OpenCode, Codex 또는 파일을 편집하고 패키지 관리자를 실행할 수 있는 모든 에이전트에서 동작합니다.

## 시작하기 전에

대상이 될 sigiro 서버가 필요합니다. `sigiro serve`로 서버를 시작하거나, [퀵스타트](/docs/tutorials/quickstart#1-run-it)의 Docker 명령으로 시작하십시오. 자체 호스팅 서버에는 키가 필요하지 않습니다. 호스팅 서비스에만 키가 필요하며, 해당 키는 `sk_<tenant>_<random>` 형식입니다.

자동 계측은 Python, Node.js, Java, .NET, Ruby용으로 제공됩니다. 서비스가 Go로 작성된 경우에는 이 프롬프트를 사용하지 마십시오. 대신 [Go 섹션](#go-instrument-the-build-not-the-source)으로 이동하십시오.

## 1. 에이전트에 다음 프롬프트를 전달하십시오

엔드포인트를 교체하십시오. 호스팅 서비스를 사용하지 않는 경우 키 줄을 삭제하십시오.

```
Instrument this codebase for sigiro observability.

SIGIRO ENDPOINT: http://localhost:4318  (self-hosted; for hosted sigiro use
                 https://<your-sigiro-host> with no port)
HOSTED API KEY: sk_<tenant>_<random>  (omit entirely for a self-hosted server)

Goal: add OpenTelemetry auto-instrumentation so this service emits traces,
metrics, and logs to sigiro. Make the minimum change that works — prefer
auto-instrumentation over hand-written spans.

Rules:
- Detect the language and framework automatically
- Use official OTel auto-instrumentation where it exists (Python opentelemetry-instrument,
  Java javaagent, Node.js auto-instrumentations-node, etc.)
- If auto-instrumentation is not available for this language/framework, add
  manual OTel SDK spans for HTTP handlers and database calls
- Set OTEL_EXPORTER_OTLP_ENDPOINT to the SIGIRO ENDPOINT above exactly as given.
  A self-hosted server listens on :4318; a hosted one carries no port. Do not add
  or remove a port
- For hosted sigiro only, set OTEL_EXPORTER_OTLP_HEADERS with the tenant bearer token
- Set OTEL_SERVICE_NAME to identify this service (use the existing service name or repo name)
- Use the existing package manager (pip, npm, Maven, Gradle, gem, go get)
- For hosted sigiro, do NOT hardcode the tenant key — read it from SIGIRO_API_KEY
- If a file already initializes the OTel SDK, update it rather than re-initializing
- Add SIGIRO_API_KEY to .env.example only for a hosted deployment
```

에이전트가 언어를 잘못 감지한 경우, 다음과 같이 직접 알려주십시오.

```
Language: <python|nodejs|java|dotnet|ruby>
Framework: <fastapi|express|spring-boot|rails|...>
```

## 2. 에이전트에게 하지 말아야 할 일을 알려주십시오

에이전트는 여기서 필요 이상의 작업을 수행합니다. 에이전트가 시작하기 전에 다음 제한 사항을 명시하십시오.

- OTel Collector를 설치하지 마십시오. sigiro가 컬렉터입니다.
- 각 엔드포인트마다 수동 스팬을 추가하지 마십시오. 자동 계측이 이를 처리합니다.
- 데이터베이스 연결 문자열이나 비즈니스 로직을 변경하지 마십시오.
- 대시보드나 알림 설정을 구성하지 마십시오. sigiro에는 둘 다 없습니다.

전체 작업은 다음과 같습니다. 프레임워크를 감지하고, 자동 계측을 설치하고, sigiro를 가리키도록 설정하는 것입니다.

## 3. 확보되는 커버리지를 확인하십시오

| 언어 | 접근 방식 | 커버리지 |
|----------|----------|--------|
| Python | `opentelemetry-instrument` CLI + distro | HTTP(FastAPI, Flask, Starlette), DB(psycopg2, asyncpg), Redis |
| Node.js | `@opentelemetry/auto-instrumentations-node` | HTTP(Express, Fastify), DB(pg, mysql2, mongoose) |
| Java | `opentelemetry-javaagent.jar` | HTTP(서블릿, Spring), DB(JDBC), JVM 메트릭 |
| .NET | `OpenTelemetry.AutoInstrumentation` 시작 훅 | HTTP(ASP.NET Core), DB(EF Core) |
| Ruby | `opentelemetry-instrumentation-all` gem | HTTP(Rails), DB(ActiveRecord), Sidekiq |
| Go | `otelc` 컴파일 타임 계측 | HTTP 서버 스팬, Go 런타임 메트릭, 서드파티 라이브러리 |

특정 라이브러리에서 스팬이 생성되지 않는 경우, 해당 라이브러리에 한해서만 수동으로 스팬을 추가하십시오.

자동 계측의 커버리지는 위 표에서 시사하는 것보다 좁으므로, 다음 두 가지 공백은 미리 예상해 두는 것이 좋습니다.

**로그.** 기본 제공되는 로그 브리지는 이름이 지정된 로거를 대상으로 합니다. Node에서는 pino, winston, bunyan이며, Python에서는 표준 `logging` 모듈입니다. `console.log`나 `print`로 기록하는 서비스는 트레이스와 메트릭은 생성하지만 **로그는 생성하지 않습니다**. 직접 브리지를 작성하기보다 제대로 된 로거로 전환하십시오.

**공식 계측이 인식하지 못하는 데이터베이스.** 위 표의 각 DB 항목은 특정 드라이버를 명시합니다. `instrumentation-pg`는 `node-postgres`를 패치하므로, `postgres.js`를 사용하는 Node 서비스에서는 쿼리 스팬이 생성되지 않으며, ORM도 이 공백을 메우지 못합니다. `drizzle-orm`은 `drizzle-orm/tracing`에 트레이서를 포함하지만, 배포된 패키지에서 해당 트레이서의 `await import('@opentelemetry/api')`가 주석 처리되어 있어 스팬을 생성할 수 없습니다. DB 열의 내용을 신뢰하기 전에 사용 중인 드라이버가 목록에 있는지 확인하고, 수동으로 스팬을 작성하기보다 해당 ORM에 대해 유지 관리되는 래퍼를 사용하십시오.

## Go: 소스가 아니라 빌드를 계측하십시오

Go에 대해서는 이 작업을 에이전트에게 맡기지 마십시오. Go에서는 빌드 명령에 접두사를 추가하면 계측된 바이너리가 생성됩니다. 소스 파일은 전혀 변경하지 않습니다. 사용하는 도구는 `otelc`이며, OpenTelemetry 프로젝트에서 빌드합니다. 이 도구는 사용자가 제어할 수 없는 서드파티 라이브러리까지 계측하는데, 에이전트는 자신의 코드만 편집하므로 이를 수행할 수 없습니다.

[v1.0.1 릴리스](https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation/releases/tag/v1.0.1)에서 사용 중인 플랫폼용 바이너리를 다운로드하십시오. 이 릴리스는 arm64 및 x86-64 기반 macOS와 Linux, 그리고 x86-64 기반 Windows용 `otelc`를 배포합니다.

```bash
curl -fsSL -o otelc https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation/releases/download/v1.0.1/otelc-darwin-arm64
chmod +x otelc
./otelc version
```

이 도구에는 `go install`이 동작하지 않습니다. 해당 모듈은 커맨드 경로를 배포하지 않습니다. 에셋이 제공되지 않는 플랫폼에서는 저장소를 클론한 뒤 `make build`를 실행하십시오.

그다음 평소 사용하는 빌드 명령 앞에 `otelc`를 추가하십시오.

```bash
./otelc go build -o checkout .
```

첫 번째 빌드에서는 OpenTelemetry 패키지를 다운로드하므로 일반 빌드보다 시간이 더 오래 걸립니다. `go.mod` 파일은 변경되지 않습니다.

[퀵스타트](/docs/tutorials/quickstart#2-send-it-telemetry)에서 사용하는 것과 동일한 두 개의 변수로 바이너리를 시작하십시오.

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_SERVICE_NAME=checkout \
  ./checkout
```

이 바이너리는 시작 시 트레이스 프로바이더, 미터 프로바이더, 로거 프로바이더를 생성합니다. 사용자가 작성한 SDK 코드는 필요하지 않습니다. 서비스는 HTTP 서버 스팬과 Go 런타임 메트릭을 보고합니다.

두 가지 제한 사항이 적용됩니다. 이 도구에는 Go 1.25 이상이 필요하며, 현재 버전은 1.0.1로 프로젝트가 2026년 7월 14일에 태그한 버전입니다. 이 도구가 다루는 라이브러리는 [업스트림 문서](https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation)를 참고하십시오.

## 4. 정상 동작을 확인하십시오

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_SERVICE_NAME=my-service \
  <your-app-start-command>
```

그런 다음 sigiro가 어떤 서비스를 수신했는지 조회하십시오.

```bash
curl http://localhost:9999/v1/services
```

실제 트래픽이 발생한 후 몇 초 이내에 서비스가 나타납니다. 서비스가 나타난 후에는 `sigiro diagnose my-service`가 읽을 데이터가 확보됩니다.

서비스가 나타나지 않으면 다음 사항을 확인하십시오. 자체 호스팅 서버에서는 포트를 확인하십시오. HTTP는 `:4318`, gRPC는 `:4317`입니다. 호스팅 서비스에서는 그 반대로, 엔드포인트에 포트가 **없는지** 확인하고, localhost가 아니라 `https://<your-sigiro-host>/v1/services`를 키와 함께 조회하십시오. `sigiro status`를 실행하여 서버가 정상 상태인지 확인하십시오. 잘못된 익스포터 설정은 부팅 시점에 오류를 보고하므로, 서비스 자체의 로그에서 OTel 시작 오류를 확인하십시오.

호스팅 서비스에서는 헤더가 정확히 `Authorization: Bearer sk_<tenant>_<random>` 형식인지 확인하십시오. 자체 호스팅 서버는 모든 인증 헤더를 무시합니다. 따라서 키가 잘못되어도 메시지나 401 응답 없이 실패합니다.

## 다음 단계

- [퀵스타트](/docs/tutorials/quickstart#3-ask-what-changed) — 무엇이 어떻게 왜 변경되었는지 질의하기
- [에이전트 자체의 텔레메트리를 sigiro로 전송하기](/docs/how-to/agent-telemetry) — Claude Code나 Codex도 sigiro를 가리키도록 설정하기
- [호스팅 sigiro로 텔레메트리 전송하기](/docs/how-to/hosted-onboarding) — 서버를 직접 운영하고 싶지 않은 경우 사용
