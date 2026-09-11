---
title: 호스티드 sigiro로 텔레메트리 전송
description: >-
  테넌트 키로 호스티드 sigiro에 OpenTelemetry를 전송한 뒤 SQL로 조회합니다. 운영할 인프라는 없으며, 초대 기반 베타
  기간에는 저희가 각 테넌트를 설정합니다.
sidebar:
  order: 2
---
이 가이드는 저희가 운영하는 sigiro 호스트로 OpenTelemetry를 전송하는 방법과
데이터를 다시 조회하는 방법을 설명합니다. 직접 운영해야 하는 인프라는 없습니다.

sigiro는 트레이스, 로그, 메트릭, 프로파일을 읽습니다. 초대 기반 베타 기간에는 각 테넌트를
저희가 직접 설정합니다. 아래의 `<your-sigiro-host>`를 운영자가 제공한 호스트로
바꾸십시오.

## 1. API 키 발급받기

운영자가 `sk_<yourtenant>_<random>` 형식의 키를 전달합니다. 이 키는 비밀번호처럼
취급하십시오. sigiro는 키를 단 한 번만 표시합니다. 키를 복구할 수는 없지만
회전(rotate)할 수 있습니다. 이 키는 인제스트와 쿼리 **양쪽**을 인증합니다.

## 2. OpenTelemetry 익스포터를 sigiro로 지정하기

HTTP 기반의 표준 OTLP를 사용하십시오. 인증에는 표준
`Authorization: Bearer` 헤더를 사용합니다.

**OTLP/HTTP:**

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="https://<your-sigiro-host>"
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer sk_<yourtenant>_<random>"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
```

**엔드포인트에는 포트를 붙이지 않습니다.** 호스티드 sigiro는 표준 HTTPS 포트에서
OTLP를 종료하므로 `https://api.sigiro.com` 자체가 전체 엔드포인트입니다. 셀프 호스팅
[퀵스타트](/docs/tutorials/quickstart#2-send-it-telemetry)의 `:4318`을 복사하지 마십시오.
`api.sigiro.com`에서는 해당 포트가 연결을 거부하며, 연결에 실패한 SDK는 그 실패를
여기가 아니라 여러분의 서비스 로그에 기록합니다. 호스트가 다른 경우에는
운영자에게 문의하십시오.

**`api.sigiro.com`에서는 OTLP/gRPC를 사용할 수 없습니다.** 포트 `4317`은 닫혀 있고
gRPC 서비스 경로도 라우팅되지 않습니다. HTTP를 사용하시고, 다른 호스트에서 gRPC를
쓸 수 있다고 가정하기 전에 운영자에게 문의하십시오.

SDK는 각 시그널을 해당 표준 OTLP 경로(`/v1/traces`, `/v1/logs`,
`/v1/metrics`, `/v1development/profiles`)로 전송하며, 이 경로를 위 엔드포인트 뒤에 덧붙입니다.
익스포터가 시그널별 엔드포인트를 받는 방식이라면 전체 경로를 지정하십시오. HTTP는
gzip을 지원하며 엔드포인트는 TLS를 사용합니다. 하나의 요청은 압축 해제 후
**8 MB 이하**여야 합니다. 기본 SDK 배치 크기는 이 한도보다 충분히 작습니다.

## 3. 데이터 수신 확인하기

텔레메트리를 전송하십시오. 플러시 간격이 약 1초이므로 몇 초 정도 기다리십시오.
그런 다음 다음 쿼리를 실행하십시오:

```bash
curl -s "https://<your-sigiro-host>/v1/query" \
  -H "Authorization: Bearer sk_<yourtenant>_<random>" \
  --data 'SELECT count(*) AS n FROM sigiro_spans'
```

`n`이 0이 아니면 트레이스가 수신되고 있다는 뜻입니다. 첫 배치를 보낸 *다음에만* 쿼리하십시오.
데이터를 한 번도 전송하지 않은 신규 테넌트에는 스토리지가 없습니다.

## 4. 데이터 쿼리하기 (SQL API)

- **엔드포인트:** 메인 HTTPS 포트의 `POST /v1/query`.
- **인증:** `Authorization: Bearer sk_<yourtenant>_<random>`.
- **본문:** JSON이 아닌 원시 SQL 문자열.
- **응답:** 행 객체로 구성된 JSON 배열. `x-sigiro-truncated: true|false`
  헤더는 sigiro가 결과를 제한했는지 여부를 알려줍니다.

각 테넌트는 격리된 카탈로그를 가지므로 **자신의** 데이터만 볼 수 있습니다.

**테이블:** `sigiro_spans`, `sigiro_logs`, `sigiro_log_templates`,
`sigiro_metrics_gauge`, `sigiro_metrics_sum`, `sigiro_metrics_histogram`,
`sigiro_metrics_exp_histogram`, `sigiro_profiles`, 그리고 `sigiro_anomalies`
테이블. `sigiro_anomalies` 테이블은 미리 계산된 국면 전환(regime shift)을 담고 있습니다. 이상 탐지 패스가
이 전환을 지속적으로 기록합니다. `GET /v1/anomalies`는 동일한 전환을 타입이 지정된
JSON으로 제공합니다. 속성 컬럼(`*_attributes`, `events_json` 등)은 JSON 텍스트를 담습니다.
필드를 읽으려면 `json_extract(col, '$.key')` 또는 `col ->> 'key'`를 사용하십시오.

각 계열이 무엇을 측정하는지, 그리고 확신에 찬 오답을 만들어내는 한 가지 이름 충돌에 대해서는
[테이블 소개](/docs/explanation/tables)를 참고하십시오.

**허용되는 SQL:** `sigiro_*` 테이블에 대한 `SELECT`만 가능합니다. sigiro는 쓰기와
DDL을 거부합니다. 또한 파일, URL, S3 리더(`read_csv`,
`read_parquet`, `glob` 등)를 차단합니다. 조인, 윈도 함수, 집계 함수,
`json_extract`는 모두 동작합니다.

**CTE는 거부됩니다.** `WITH` 절은 그 내용이 무엇이든 검증에 실패합니다.
파생 테이블 서브쿼리로 다시 작성하십시오: `SELECT ... FROM (SELECT ...) t`.
하나의 공유 카탈로그가 모든 테넌트를 담고 있으므로, 호스티드 서비스는
`information_schema`도 차단합니다.

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

| 제한 | 값 | 초과 시 sigiro의 동작 |
|-------|-------|-----------|
| 쿼리 타임아웃 | 10초 | sigiro가 쿼리를 거부합니다 (400) |
| 결과 행 수 | 100,000 | sigiro가 응답을 절단하고 `x-sigiro-truncated: true`를 설정합니다 |
| 쿼리 메모리 | 약 400 MB | 쿼리가 실패합니다 (400) |
| 인제스트 본문 | 8 MB (압축 해제 기준) | 413 (HTTP) / RESOURCE_EXHAUSTED (gRPC) |
| SQL/API 요청 속도 | 테넌트당 5 req/s, 버스트 20 | 429 |
| OTLP/HTTP 요청 속도 | 소스당 50 req/s, 버스트 100 | 429 |

sigiro에는 보조 인덱스가 없습니다. 따라서 **모든 쿼리를 `timestamp`로 제한하십시오**.
제한을 두면 속도가 빨라집니다. 또한 타임아웃과 절단도 방지됩니다.

## 6. 데이터 보존

베타 기간에는 sigiro를 기록 시스템이 아니라 실시간 쿼리 서비스로 취급하십시오.
현재 보존 기간은 운영자에게 문의하십시오. 장기간 보관해야 하는 데이터는
내보내 두십시오.

## 다음 단계

- [에이전트로 코드 계측하기](/docs/how-to/install-with-ai) — 서비스에
  아직 OpenTelemetry가 없는 경우
- [API 참조](/docs/reference) — 다섯 개의 엔드포인트, 각각에 요청
  플레이그라운드 포함
- [대시보드 대신 증거를 제공하는 이유](/docs/explanation/evidence) —
  `/v1/diagnose`가 차트가 아니라 순위가 매겨진 목록을 반환하는 이유
