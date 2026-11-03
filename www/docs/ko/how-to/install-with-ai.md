---
title: 에이전트로 코드 계측하기
description: 에이전트에게 프롬프트 하나를 주면 서비스에 OpenTelemetry 자동 계측을 추가하고 sigiro로 향하게 합니다. Claude Code, OpenCode, Codex에서 동작합니다.
sidebar:
  order: 1
---

이 가이드는 OpenTelemetry가 전혀 없는 서비스에 에이전트가 편집을 맡아 OpenTelemetry를 추가하는 방법을 보여 줍니다. 아래 프롬프트를 에이전트에 붙여 넣으십시오. 에이전트가 프레임워크를 감지하고, 공식 자동 계측을 설치하고, 익스포터를 sigiro로 향하게 합니다.

Claude Code, OpenCode, Codex 또는 파일을 편집하고 패키지 매니저를 실행할 수 있는 모든 에이전트에서 동작합니다.

## sigiro 스킬 설치하기

에이전트를 시작하기 전에 공개 스킬을 설치하십시오.

```bash
npx skills add Arnav12114/skills
```

[`debug-with-sigiro` 스킬](https://github.com/Arnav12114/skills)은 텔레메트리가 도착한 뒤 sigiro를 사용하는 방법을 에이전트에게 알려 줍니다. 이 가이드는 계측에만 집중합니다.

## 시작하기 전에

목적지로 sigiro 서버가 필요합니다. `sigiro serve`로 시작하거나 [빠른 시작](/docs/tutorials/quickstart#1-실행하기)의 Docker 명령으로 시작하십시오. 호스팅 sigiro의 경우 에이전트를 시작하기 전에 새 계정은 `sigiro signup --name "Your Name" --email you@example.com`을(기존 계정은 브라우저 기반 `sigiro auth login`을) 실행하십시오. 셀프 호스팅 서버에는 키가 필요하지 않습니다.

자동 계측은 Python, Node.js, Java, .NET, Ruby용으로 제공됩니다. 서비스가 Go로 작성된 경우에는 이 프롬프트를 사용하지 마십시오. 대신 [Go 섹션](#go-소스가-아니라-빌드를-계측하십시오)으로 이동하십시오.

## 1. 에이전트에게 이 프롬프트 주기

엔드포인트를 바꾸십시오. 새 호스팅 계정이라면 먼저 터미널 가입 명령을 실행하십시오. 기존 계정이라면 토큰을 프롬프트에 복사하지 말고 로그인된 CLI 세션을 사용하십시오.

```
Instrument this codebase for sigiro observability.

SIGIRO ENDPOINT: http://localhost:4318  (self-hosted; for hosted sigiro use
                 https://<your-sigiro-host> with no port)
HOSTED AUTH: sigiro signup --name "Your Name" --email you@example.com  (new account;
             use sigiro auth login for an existing account; omit for self-hosted)

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
- For hosted sigiro only, read a bearer token with `sigiro auth token`; never paste it into source
- Set OTEL_SERVICE_NAME to identify this service (use the existing service name or repo name)
- Use the existing package manager (pip, npm, Maven, Gradle, gem, go get)
- For a new hosted account, use `sigiro signup --name "Your Name" --email you@example.com`;
  for an existing account, use `sigiro auth login`. Never hardcode credentials.
- If a file already initializes the OTel SDK, update it rather than re-initializing
- For automation only, read credentials from the environment; interactive users should
  use the OS credential store via `sigiro signup` or `sigiro auth login`
```

에이전트가 언어를 잘못 감지하면 직접 알려 주십시오.

```
Language: <python|nodejs|java|dotnet|ruby>
Framework: <fastapi|express|spring-boot|rails|...>
```

## 2. 에이전트에게 하지 말아야 할 일 알려 주기

이 작업에서 에이전트는 원하는 것보다 많은 일을 합니다. 에이전트가 시작하기 전에 다음 제한을 말해 주십시오.

- OTel Collector를 설치하지 마십시오. sigiro가 컬렉터입니다.
- 엔드포인트마다 수동 스팬을 추가하지 마십시오. 자동 계측이 처리합니다.
- 데이터베이스 연결 문자열이나 비즈니스 로직을 바꾸지 마십시오.
- 대시보드나 알림 설정을 만들지 마십시오. 이 작업에 포함되지 않습니다.

작업 전체는 프레임워크를 감지하고, 자동 계측을 설치하고, 그것을 sigiro로 향하게 하는 것입니다.

## 3. 얻는 커버리지 확인하기

| 언어    | 방식                                             | 대상                                                            |
| ------- | ------------------------------------------------ | --------------------------------------------------------------- |
| Python  | `opentelemetry-instrument` CLI + distro          | HTTP (FastAPI, Flask, Starlette), DB (psycopg2, asyncpg), Redis |
| Node.js | `@opentelemetry/auto-instrumentations-node`      | HTTP (Express, Fastify), DB (pg, mysql2, mongoose)              |
| Java    | `opentelemetry-javaagent.jar`                    | HTTP (Servlets, Spring), DB (JDBC), JVM 메트릭                  |
| .NET    | `OpenTelemetry.AutoInstrumentation` startup hook | HTTP (ASP.NET Core), DB (EF Core)                               |
| Ruby    | `opentelemetry-instrumentation-all` gem          | HTTP (Rails), DB (ActiveRecord), Sidekiq                        |
| Go      | `otelc` 컴파일 타임 계측                         | HTTP 서버 스팬, Go 런타임 메트릭, 서드 파티 라이브러리          |

어떤 라이브러리에서 스팬이 나오지 않으면 그 라이브러리에 대해서만 스팬을 직접 추가하십시오.

자동 계측은 표가 암시하는 것보다 적게 다루므로, 두 가지 빈틈을 예상해 두는 것이 좋습니다.

**로그.** 번들된 로그 브리지는 이름 있는 로거를 다룹니다. Node에서는 pino, winston, bunyan, Python에서는 표준 `logging` 모듈입니다. `console.log`나 `print`로 쓰는 서비스는 트레이스와 메트릭은 만들지만 **로그는 만들지 않습니다**. 직접 브리지하지 말고 제대로 된 로거로 옮기십시오.

**공식 계측이 모르는 데이터베이스.** 위의 각 DB 항목은 특정 드라이버를 지목합니다. `instrumentation-pg`는 `node-postgres`를 패치하므로 `postgres.js`를 쓰는 Node 서비스에는 쿼리 스팬이 생기지 않으며, ORM도 그 빈틈을 메우지 못합니다. `drizzle-orm`은 `drizzle-orm/tracing`에 트레이서를 포함하지만 게시된 패키지에서 `await import('@opentelemetry/api')`가 주석 처리되어 있어 스팬을 내보낼 수 없습니다. DB 열을 믿기 전에 자신의 드라이버가 목록에 있는지 확인하고, 직접 작성한 스팬보다 그 ORM용으로 유지 관리되는 래퍼를 선호하십시오.

## Go: 소스가 아니라 빌드를 계측하십시오

Go에서는 이 일을 에이전트에게 맡기지 마십시오. Go에서는 빌드 명령에 접두사를 붙이면 계측된 바이너리가 나옵니다. 소스 파일은 바꾸지 않습니다. 도구는 `otelc`이며 OpenTelemetry 프로젝트가 빌드합니다. 이 도구는 사용자가 제어하지 않는 서드 파티 라이브러리도 계측합니다. 에이전트는 사용자 자신의 코드만 편집하므로 이는 에이전트가 할 수 없는 일입니다.

[v1.0.1 릴리스](https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation/releases/tag/v1.0.1)에서 플랫폼에 맞는 바이너리를 내려받으십시오. 이 릴리스는 macOS와 Linux의 arm64 및 x86-64, Windows의 x86-64용 `otelc`를 제공합니다.

```bash
curl -fsSL -o otelc https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation/releases/download/v1.0.1/otelc-darwin-arm64
chmod +x otelc
./otelc version
```

이 도구에는 `go install`이 동작하지 않습니다. 모듈이 명령 경로를 게시하지 않기 때문입니다. 에셋이 없는 플랫폼에서는 저장소를 클론하고 `make build`를 실행하십시오.

그런 다음 평소 빌드 명령 앞에 `otelc`를 붙이십시오.

```bash
./otelc go build -o checkout .
```

첫 빌드는 OpenTelemetry 패키지를 내려받으므로 평소 빌드보다 오래 걸립니다. `go.mod` 파일은 바뀌지 않습니다.

[빠른 시작](/docs/tutorials/quickstart#2-텔레메트리-전송하기)과 같은 두 변수로 바이너리를 시작하십시오.

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_SERVICE_NAME=checkout \
  ./checkout
```

바이너리는 시작할 때 트레이스 프로바이더, 미터 프로바이더, 로거 프로바이더를 만듭니다. 사용자의 SDK 코드가 필요하지 않습니다. 서비스는 HTTP 서버 스팬과 Go 런타임 메트릭을 보고합니다.

제한이 두 가지 있습니다. 이 도구는 Go 1.25 이상이 필요하며, 현재 버전은 프로젝트가 2026년 7월 14일에 태그한 1.0.1입니다. 다루는 라이브러리는 [업스트림 문서](https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation)를 읽으십시오.

## 4. 동작 확인하기

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_SERVICE_NAME=my-service \
  <your-app-start-command>
```

그런 다음 sigiro가 어떤 서비스를 받았는지 물어보십시오.

```bash
curl http://localhost:9999/v1/services
```

실제 트래픽이 있으면 몇 초 안에 서비스가 나타납니다. 나타난 뒤에는 범위가 정해진 `POST /v1/query`로 스팬과 로그를 살펴볼 수 있습니다.

서비스가 나타나지 않으면 다음을 확인하십시오. 셀프 호스팅 서버에서는 포트를 확인하십시오. HTTP는 `:4318`, gRPC는 `:4317`입니다. 호스팅 서비스에서는 반대로 엔드포인트에 포트가 **없는지** 확인하고, localhost가 아니라 `https://<your-sigiro-host>/v1/services`를 키와 함께 쿼리하십시오. `sigiro status`를 실행해 서버가 정상인지 확인하십시오. 잘못된 익스포터 설정은 부팅 시 문제를 보고하므로, 자신의 서비스 로그에서 OTel 시작 오류를 읽으십시오.

호스팅 서비스에서는 헤더가 정확히 `Authorization: Bearer <oauth-access-token>`인지 확인하십시오. 셀프 호스팅 서버는 모든 인증 헤더를 무시합니다. 따라서 잘못된 키는 메시지도 401 응답도 없이 실패합니다.

## 다음 단계

- [빠른 시작](/docs/tutorials/quickstart#3-최근-오류-조회) — 최근 오류를 SQL로 조회합니다
- [에이전트 자체 텔레메트리를 sigiro로 전송하기](/docs/how-to/agent-telemetry) — Claude Code나 Codex도 sigiro로 향하게 합니다
- [호스티드 sigiro로 텔레메트리 전송](/docs/how-to/hosted-onboarding) — 서버를 직접 운영하고 싶지 않다면 사용하십시오
