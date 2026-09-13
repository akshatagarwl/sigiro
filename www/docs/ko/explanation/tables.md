---
title: 텔레메트리 테이블 이해하기
description: 각 sigiro_* 테이블의 데이터와 텔레메트리를 보낸 머신을 조회하는 방법입니다.
sidebar:
  order: 2
---

sigiro는 OTLP로 전송된 텔레메트리를 저장합니다. 각 행은 해당 텔레메트리를 보낸 서비스와 머신을 설명합니다. Sigiro 서버가 실행되는 머신의 실시간 호스트 정보는 노출하지 않습니다.

## 저장되는 신호

- `sigiro_spans` — 트레이스와 스팬 시간
- `sigiro_logs` — 로그 레코드
- `sigiro_log_templates` — 중복 제거된 로그 템플릿
- `sigiro_metrics_gauge` — 특정 시점의 값
- `sigiro_metrics_sum` — 단조 및 비단조 합계
- `sigiro_metrics_histogram` — 명시적 버킷 히스토그램
- `sigiro_metrics_exp_histogram` — 지수 히스토그램
- `sigiro_profiles` — 프로파일 샘플
- `sigiro_anomalies` — 감지된 체제 변화

OTLP가 서로 다른 네 가지 메트릭 형태를 정의하므로 메트릭은 네 테이블에 저장됩니다. 여러 메트릭 유형이 필요하면 `UNION ALL`을 사용합니다.

## 올바른 머신 조회하기

CPU, 메모리, 파일 시스템, 네트워크 질문은 `sigiro_metrics_*` 테이블을 조회합니다. 결과는 Sigiro 실행 머신이 아니라 메트릭을 내보낸 머신에 대한 값입니다.

```sql
SELECT service_name, metric_name, value, timestamp
FROM sigiro_metrics_gauge
WHERE service_name = 'checkout'
  AND metric_name LIKE 'system.filesystem%'
  AND timestamp >= TIMESTAMP '2026-09-12 00:00:00'
  AND timestamp <  TIMESTAMP '2026-09-13 00:00:00'
ORDER BY timestamp DESC;
```

결과가 비어 있으면 해당 서비스가 일치하는 호스트 메트릭을 보내지 않은 것입니다. 다른 머신의 데이터로 대체하지 말고 계측을 수정하세요.

## 쿼리 규칙

원시 텔레메트리 쿼리에는 유한한 반개방 시간 범위인 `timestamp >= ... AND timestamp < ...`가 필요합니다.

## 다음 문서

- [대시보드 대신 증거를 사용하는 이유](/ko/docs/explanation/evidence)
- [호스팅 sigiro로 텔레메트리 보내기](/ko/docs/how-to/hosted-onboarding#4-query-your-data-sql-api)
