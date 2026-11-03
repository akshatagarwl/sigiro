---
title: 호스티드 sigiro로 텔레메트리 전송
description: 터미널에서 호스팅 sigiro 계정을 만들고, OpenTelemetry를 보내고, SQL로 다시 조회합니다.
sidebar:
  order: 3
---

이 가이드는 저희가 운영하는 sigiro 호스트로 OpenTelemetry를 보내는 방법과 그 데이터를 다시 조회하는 방법을 설명합니다. 인프라를 운영할 필요가 없습니다.

처음부터 끝까지의 흐름은 다음과 같습니다.

1. [sigiro.com/sign-in](https://sigiro.com/sign-in)에서 가입하거나 터미널에서 `sigiro signup`을 실행합니다. 어느 쪽이든 개인 워크스페이스 하나가 생깁니다.
2. CLI를 설치하고 그 워크스페이스에 연결합니다(`sigiro auth login`).
3. 아래 `OTEL_*` 변수를 내보내고 앱을 시작합니다. 앱은 `https://sigiro.com`으로 보냅니다. 별도의 수집 호스트는 없습니다.
4. [sigiro.com/dashboard](https://sigiro.com/dashboard/)에서 데이터를 보거나 SQL로 조회합니다.

sigiro는 트레이스, 로그, 메트릭, 프로파일을 읽습니다. 먼저 CLI를 설치하십시오. macOS, Linux, WSL에서 명령 하나입니다.

```bash
curl -fsSL https://sigiro.com/install | sh
```

그런 다음 CLI를 계정에 연결합니다. 실행할 명령은 어디서 시작했는지에 따라 다르며, 아래 [CLI 연결하기](#1-cli-연결하기)에 둘 다 있습니다.

## 1. CLI 연결하기

**이미 웹에서 가입을 마쳤다면.** 설치한 CLI로 기존 계정에 로그인하십시오.

```bash
sigiro auth login
sigiro auth status
```

`sigiro auth login`은 OAuth 디바이스 인증을 시작하고 확인 페이지를 브라우저로 엽니다. 그 머신에서 브라우저를 열 수 없다면 `--no-browser`를 주십시오. 어느 경우든 CLI가 확인할 URL과 코드를 출력합니다.

**아직 계정이 없고 터미널에서 시작한다면.** 셸을 떠나지 않고 계정을 만드십시오.

```bash
sigiro signup --name "Your Name" --email you@example.com
sigiro auth status
```

`sigiro signup`은 여섯 자리 OTP를 이메일로 보내고 터미널에서 안전하게 입력받습니다. 개인 워크스페이스를 정확히 하나 만들어 활성화하고, 브라우저 없이 OAuth 디바이스 인증을 내부에서 완료하고, 리프레시/액세스 자격 증명을 운영 체제의 자격 증명 저장소에 저장합니다.

**모든 로그인은 워크스페이스 하나에 묶이며, 브라우저가 그 이름을 보여 줍니다.** 승인 페이지에 CLI가 사용할 워크스페이스가 표시됩니다. 계정이 여러 워크스페이스에 속해 있다면 그곳에서 고르며, 고르기 전에는 아무것도 승인되지 않습니다. 이 선택은 로그인의 일부입니다. 세션을 갱신해도, 나중에 다른 워크스페이스에 가입해도 같은 워크스페이스가 유지되며, 그 워크스페이스에서 제거되면 로그인이 끝납니다. 바꾸려면 `sigiro auth login`을 다시 실행하십시오.

웹 가입과 `sigiro signup`은 같은 프로비저닝 단계를 호출하며, 계정에 워크스페이스가 없을 때만 개인 워크스페이스를 만듭니다. 따라서 이미 있는 계정에 대해 `sigiro signup`은 멱등입니다. 그 계정에 로그인할 뿐 두 번째 계정이나 두 번째 워크스페이스를 만들지 않습니다. 이미 여러 워크스페이스에 속한 계정이라면 멈추고 `sigiro auth login`을 실행하라고 안내하므로, 대신 하나를 고르는 일은 없습니다.

`sigiro auth status`는 CLI가 묶인 계정과 워크스페이스를 출력합니다. `sigiro auth logout`은 저장된 자격 증명을 폐기하고 삭제합니다. 두 흐름 모두 자격 증명을 셸 기록에 남기지 않으며, OAuth API를 호출하거나 토큰을 손으로 복사할 필요가 없습니다.

여기서는 사람의 가입과 OAuth 로그인을 다룹니다. 소프트웨어에 자체 위임 ID와 키가 필요하다면 [Agent Auth 온보딩](/docs/how-to/agent-auth)을 사용하십시오.

호스팅 인증은 Better Auth가 제공합니다. 워크스페이스는 격리 단위입니다. 데이터, 멤버, 쿼리가 다른 모든 워크스페이스와 분리됩니다. 일부 API 필드와 오류 메시지는 이를 테넌트나 조직이라고 부르지만, 세 단어 모두 같은 것을 가리킵니다.
두 명령 모두 기본적으로 호스팅 서비스 `https://sigiro.com`을 대상으로 합니다.

같은 토큰이 OTLP 수집과 API 쿼리를 모두 인증합니다.

## 2. OpenTelemetry 익스포터를 sigiro로 지정하기

표준 OTLP over HTTP를 사용하십시오. 인증에는 표준 `Authorization: Bearer` 헤더를 사용합니다.

**OTLP/HTTP:**

```bash
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer $(sigiro auth token)"
export OTEL_EXPORTER_OTLP_ENDPOINT="https://sigiro.com"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
export OTEL_SERVICE_NAME="checkout"   # the name your service shows up under
```

헤더를 먼저 설정하십시오. `OTEL_EXPORTER_OTLP_ENDPOINT`가 이미 내보내져 있으면 `sigiro` CLI가 자체 텔레메트리를 그곳으로 보내려 하면서 `SIGIRO_OTEL_AUTH_KEY is required` 경고를 출력합니다. 이 경고는 무해하며 토큰은 그대로 출력됩니다.

**토큰은 15분 후 만료됩니다.** `sigiro auth token`은 저장된 세션을 갱신한 뒤 수명이 짧은 액세스 토큰을 출력합니다. 이 토큰으로 시작한 앱은 15분 동안 데이터를 보내고, 그 뒤에는 익스포터가 "The access token has expired"라는 사유와 함께 HTTP `401`을 받습니다. 이는 앱 자체 로그에만 나타납니다. sigiro를 시험해 보기에는 이것으로 충분합니다.

**오래 실행되는 서비스는 수집 키로 보냅니다.** CLI가 로그인한 워크스페이스용 키를 만드십시오.

```bash
sigiro ingest-keys create --name checkout
```

키 `sigiro_<id>_<secret>`과 그에 맞는 `otel_exporter_otlp_headers` 값이 출력됩니다. 키는 지금 저장하십시오. 한 번만 표시되며 sigiro는 해시만 보관합니다. 그런 다음 토큰 대신 키를 사용하십시오.

```bash
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer sigiro_<id>_<secret>"
```

키는 만료되지 않습니다. 텔레메트리 전송만 할 수 있으며, 이 키로 한 쿼리는 `403`을 받습니다. `sigiro ingest-keys list`는 각 키의 ID와 마지막 사용 시각을 보여 주고, `sigiro ingest-keys revoke <id>`는 키를 즉시 중지합니다.

**앱에 아직 OpenTelemetry가 없다면.** 제로 코드 계측은 위 변수만으로 코드 변경 없이 트레이스, 로그, 메트릭을 보냅니다. Node.js:

```bash
npm install @opentelemetry/api @opentelemetry/auto-instrumentations-node
node --require @opentelemetry/auto-instrumentations-node/register app.js
```

Python:

```bash
pip install opentelemetry-distro opentelemetry-exporter-otlp-proto-http
opentelemetry-bootstrap -a install
export OTEL_PYTHON_LOGGING_AUTO_INSTRUMENTATION_ENABLED=true  # also send logs
opentelemetry-instrument python app.py
```

다른 언어이거나 에이전트에게 설정을 맡기려면 [에이전트로 코드 계측하기](/docs/how-to/install-with-ai)를 따르십시오.

**엔드포인트에는 포트가 없습니다.** 호스팅 sigiro는 표준 HTTPS 포트에서 OTLP를 종료하므로 `https://sigiro.com`이 엔드포인트 전체입니다. 셀프 호스팅 [빠른 시작](/docs/tutorials/quickstart#2-텔레메트리-전송하기)의 `:4318`을 복사하지 마십시오. `sigiro.com`에서는 그 포트가 연결을 거부하며, 연결하지 못한 SDK는 실패를 여기가 아니라 자신의 서비스 로그에 보고합니다.

**`sigiro.com`에서는 OTLP/gRPC를 사용할 수 없습니다.** 포트 `4317`은 닫혀 있고 gRPC 서비스 경로는 라우팅되지 않습니다. HTTP를 사용하십시오.

**참고 — 사용자 지정 호스트나 셀프 호스팅 호스트.** 위아래의 모든 내용은 저희가 운영하며 CLI의 기본값인 `https://sigiro.com`을 설명합니다. 사용자나 운영자가 sigiro를 다른 곳에서 운영한다면 이 예시의 호스트를 바꾸고, CLI가 그곳을 대상으로 하도록 `SIGIRO_ENDPOINT`를 설정하십시오. 그 경우 포트, gRPC 사용 가능 여부, 보존 기간은 해당 배포의 설정에 따르므로 이 페이지를 가정하지 말고 운영자에게 문의하십시오. `sigiro signup`과 `sigiro auth login`은 여전히 `https://sigiro.com`의 호스팅 서비스로 인증합니다.

SDK는 각 신호를 표준 OTLP 경로(`/v1/traces`, `/v1/logs`, `/v1/metrics`, `/v1development/profiles`)로 보내며, 이 경로를 위 엔드포인트 뒤에 붙입니다. 익스포터가 신호별 엔드포인트를 받는다면 전체 경로를 주십시오. HTTP는 gzip을 지원하고 엔드포인트는 TLS를 사용합니다. 요청 하나는 압축 해제 후 **8 MB 이하**여야 합니다. 기본 SDK 배치 크기는 이 제한보다 훨씬 작습니다.

Pyroscope SDK는 OTLP 프로파일 경로를 사용하지 않습니다. 대상, 요청 형식, 서비스 이름 규칙은 [Pyroscope 프로파일을 sigiro로 전송하기](/docs/how-to/send-profiles)를 따르십시오.

## 3. 데이터 수신 확인하기

앱으로 요청을 몇 개 보내십시오. 플러시 간격이 약 1초이므로 몇 초 기다리십시오. 그런 다음 [sigiro.com/dashboard](https://sigiro.com/dashboard/)를 열고 같은 이메일로 로그인하십시오. 서비스가 `OTEL_SERVICE_NAME` 이름으로 트레이스, 로그와 함께 표시됩니다.

대신 터미널에서 확인하려면:

```bash
curl -s "https://sigiro.com/v1/services" \
  -H "Authorization: Bearer $(sigiro auth token)"
```

서비스가 신호별 개수와 함께 `services` 배열에 나타납니다. 없다면 앱 자체 로그에서 익스포터 오류를 확인하십시오. `401`에는 사유(토큰 만료, 키 취소 또는 알 수 없음, 헤더 누락)가 담겨 있고, 연결 오류는 대개 엔드포인트에 `:4318`이 남아 있다는 뜻입니다.

**아직 계측한 앱이 없다면.** 테스트 스팬 하나를 직접 보내 경로를 확인하십시오. OTLP/HTTP는 protobuf뿐 아니라 JSON도 받습니다.

```bash
now=$(date +%s)000000000
curl -s -w '%{http_code}\n' https://sigiro.com/v1/traces \
  -H "Authorization: Bearer $(sigiro auth token)" \
  -H "content-type: application/json" \
  --data '{"resourceSpans":[{"resource":{"attributes":[{"key":"service.name","value":{"stringValue":"hello-sigiro"}}]},
    "scopeSpans":[{"spans":[{"traceId":"5b8efff798038103d269b633813fc60c","spanId":"eee19b7ec3c1b174",
    "name":"GET /hello","kind":2,"startTimeUnixNano":"'"$now"'","endTimeUnixNano":"'"$now"'"}]}]}]}'
```

`200`은 sigiro가 스팬을 받았다는 뜻입니다. 몇 초 뒤 `hello-sigiro`가 `/v1/services`에 나타납니다.

이 용도에는 `sigiro query`가 맞지 않습니다. 이 명령은 셀프 호스팅 엣지를 페더레이션하며 `sigiro.com`에서는 실패합니다. 터미널이나 에이전트에서 쿼리하려면 위처럼 `curl`을 사용하거나, 저장된 로그인으로 같은 워크스페이스를 읽는 [`sigiro mcp`](/docs/reference/cli#mcp)를 사용하십시오.

```bash
claude mcp add sigiro -- sigiro mcp
```

## 4. 데이터 쿼리하기 (SQL API)

- **엔드포인트:** 메인 HTTPS 포트의 `POST /v1/query`.
- **인증:** `Authorization: Bearer $(sigiro auth token)`.
- **본문:** JSON이 아닌 원시 SQL 문자열.
- **응답:** 행 객체의 JSON 배열. `x-sigiro-truncated: true|false` 헤더는 sigiro가 결과를 잘랐는지 알려 줍니다.

각 워크스페이스에는 격리된 카탈로그가 있으므로 **자신의** 워크스페이스 데이터만 볼 수 있습니다.

**테이블:** `sigiro_spans`, `sigiro_logs`, `sigiro_metrics_gauge`, `sigiro_metrics_sum`, `sigiro_metrics_histogram`, `sigiro_metrics_exp_histogram`, `sigiro_profiles`. 속성 열에는 JSON 텍스트가 들어 있으며, `json_extract(col, '$.key')`나 `col ->> 'key'`로 살펴볼 수 있습니다.

각 계열이 무엇을 측정하는지, 그리고 확신에 찬 오답을 만드는 유일한 이름 충돌에 대해서는 [테이블 이해하기](/docs/explanation/tables)를 읽으십시오.

**허용되는 SQL:** `sigiro_*` 테이블에 대한 `SELECT`만 허용됩니다. sigiro는 쓰기와 DDL을 거부합니다. 파일, URL, S3 리더(`read_csv`, `read_parquet`, `glob` 등)도 차단합니다. 조인, 윈도 함수, 집계, `json_extract`는 모두 동작합니다.

**CTE는 거부됩니다.** `WITH` 절은 내용과 상관없이 검증에 실패합니다. 파생 테이블 서브쿼리로 다시 쓰십시오: `SELECT ... FROM (SELECT ...) t`. 공유 카탈로그 하나가 모든 테넌트를 담고 있으므로 호스팅 서비스는 `information_schema`도 차단합니다.

**예시:**

```sql
-- Slowest operations, last hour
SELECT service_name, span_name,
       approx_quantile(duration, 0.95) / 1000.0 AS p95_ms, count(*) AS n
FROM sigiro_spans
WHERE timestamp > now() - INTERVAL '1 hour'
GROUP BY 1, 2 ORDER BY p95_ms DESC LIMIT 10;
```

```sql
-- Error-log rate per route, last 15 min
SELECT service_name, log_attributes ->> 'http.route' AS route, count(*) AS errors
FROM sigiro_logs
WHERE timestamp > now() - INTERVAL '15 minutes' AND severity_number >= 17
GROUP BY 1, 2 ORDER BY errors DESC;
```

## 5. 제한

| 제한                     | 값                                | 초과하면 sigiro가 하는 일                                          |
| ------------------------ | --------------------------------- | ------------------------------------------------------------------ |
| 쿼리 타임아웃            | 10초                              | sigiro가 쿼리를 거부합니다(400)                                    |
| 결과 행                  | 100,000                           | sigiro가 응답을 자르고 `x-sigiro-truncated: true`를 설정합니다     |
| 쿼리 메모리              | 약 400 MB                         | 쿼리가 실패합니다(400)                                             |
| 수집 본문                | 8 MB(압축 해제 후)                | 413                                                                |
| 동시 쿼리                | 테넌트당 4개                      | 추가 쿼리는 최대 5초 동안 자리를 기다린 뒤 `Retry-After`와 함께 429 |
| SQL/API 요청 속도        | 테넌트당 5 req/s, 버스트 20       | 429                                                                |
| OTLP/HTTP 요청 속도      | 소스당 50 req/s, 버스트 100       | 429                                                                |

sigiro에는 보조 인덱스가 없습니다. 따라서 **모든 쿼리를 `timestamp`로 범위를 정하십시오**. 범위를 정하면 빨라지고, 타임아웃과 잘림도 막을 수 있습니다.

## 6. 데이터 보존

베타 기간에는 sigiro를 기록 보관 시스템이 아니라 실시간 쿼리 서비스로 취급하십시오. `sigiro.com`은 아직 보존 기간을 공개하지 않았습니다. 현재 기간은 [문의해 주십시오](https://cal.com/akshatagarwl/30-minutes). 오래 보관해야 하는 데이터는 내보내십시오.

## 다음 단계

- [대시보드](https://sigiro.com/dashboard/) — 로그인한 워크스페이스의 서비스, 트레이스, 로그를 브라우저에서 봅니다
- [에이전트로 코드 계측하기](/docs/how-to/install-with-ai) — 서비스에 아직 OpenTelemetry가 없다면
- [API 참조](/docs/reference) — 서버의 `/openapi.json`이 현재 설명하는 HTTP 작업
