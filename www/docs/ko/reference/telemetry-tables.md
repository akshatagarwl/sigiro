---
title: 텔레메트리 테이블 열
description: 현재 DuckDB 열 이름, 유형, 단위 및 1.0 이전 호환성.
sidebar:
  order: 4
---

다음은 `src/schema.rs`에서 생성하는 쿼리 가능한 7개 `sigiro_*` 테이블입니다. 유형은 DuckDB SQL 유형이며 선언된 `NOT NULL`을 표시합니다. 쿼리에서는 `lake.` 접두사와 짧은 테이블 이름을 모두 사용합니다. 이는 **현재** 스키마의 참고 자료이며 런타임 스키마 검색 API가 아닙니다.

**1.0 이전 호환성:** 열 이름, 유형, JSON 형태는 1.0 이전에 바뀔 수 있습니다. 이 문서는 DuckLake/Parquet 디스크 형식의 안정성, 자동 인플레이스 업그레이드 또는 다운그레이드 지원을 보장하지 않습니다. 저장소 형식 검증과 업그레이드 정책은 별도 작업 [#200](https://github.com/akshatagarwl/sigiro/issues/200), 런타임 스키마 검색은 [#13](https://github.com/akshatagarwl/sigiro/issues/13)에서 다룹니다.

`TIMESTAMP`는 마이크로초 정밀도의 DuckDB 타임스탬프입니다. 숫자 에폭 시각(`end_timestamp`, `observed_timestamp`, 메트릭 `start_timestamp`)과 스팬 `duration`은 OTLP 나노초를 변환한 **마이크로초**입니다. 반면 `events_json` 및 `exemplars_json` 내부의 `time_unix_nano`는 **나노초** 그대로입니다. 메트릭 값과 히스토그램 경계의 단위는 전송자의 `metric_unit`, 프로파일 값은 `unit`에 따릅니다.

속성과 JSON 배열은 네이티브 JSON 열이 아닌 **JSON을 담은 `VARCHAR`**입니다. 속성 키는 `json_extract_string(column, 'key')`로 읽으세요. 키는 전송자마다 달라 이 목록에 포함되지 않습니다. DDL에서 nullable이라도 현재 수집 경로가 반드시 null을 쓰는 것은 아닙니다.

## `sigiro_spans`

| 열                         | 유형                 | 단위         | 의미                               |
| -------------------------- | -------------------- | ------------ | ---------------------------------- |
| `timestamp`                | `TIMESTAMP NOT NULL` | µs precision | 이벤트 또는 데이터 포인트 시각.    |
| `end_timestamp`            | `BIGINT NOT NULL`    | epoch µs     | 스팬 종료 시각.                    |
| `duration`                 | `BIGINT NOT NULL`    | µs           | 스팬 경과 시간.                    |
| `trace_id`                 | `VARCHAR`            | —            | 추적 ID(16진 문자열).              |
| `span_id`                  | `VARCHAR`            | —            | 스팬 ID(16진 문자열).              |
| `parent_span_id`           | `VARCHAR`            | —            | 부모 스팬 ID(16진 문자열).         |
| `trace_state`              | `VARCHAR`            | —            | W3C 추적 상태.                     |
| `service_name`             | `VARCHAR NOT NULL`   | —            | OTel 리소스의 service.name.        |
| `service_namespace`        | `VARCHAR`            | —            | OTel 리소스의 service.namespace.   |
| `service_instance_id`      | `VARCHAR`            | —            | OTel 리소스의 service.instance.id. |
| `span_name`                | `VARCHAR NOT NULL`   | —            | 스팬 작업 이름.                    |
| `span_kind`                | `INTEGER NOT NULL`   | —            | OTLP 스팬 종류 열거값.             |
| `status_code`              | `INTEGER NOT NULL`   | —            | OTLP 상태 코드 열거값.             |
| `status_message`           | `VARCHAR`            | —            | 스팬 상태 메시지.                  |
| `resource_attributes`      | `VARCHAR`            | —            | 리소스 속성의 JSON 문자열.         |
| `scope_name`               | `VARCHAR`            | —            | 계측 범위 이름.                    |
| `scope_version`            | `VARCHAR`            | —            | 계측 범위 버전.                    |
| `scope_attributes`         | `VARCHAR`            | —            | 범위 속성의 JSON 문자열.           |
| `span_attributes`          | `VARCHAR`            | —            | 스팬 속성의 JSON 문자열.           |
| `events_json`              | `VARCHAR`            | —            | 스팬 이벤트의 JSON 배열 문자열.    |
| `links_json`               | `VARCHAR`            | —            | 스팬 링크의 JSON 배열 문자열.      |
| `dropped_attributes_count` | `INTEGER`            | count        | 버려진 스팬 속성 수.               |
| `dropped_events_count`     | `INTEGER`            | count        | 버려진 스팬 이벤트 수.             |
| `dropped_links_count`      | `INTEGER`            | count        | 버려진 스팬 링크 수.               |
| `flags`                    | `INTEGER`            | —            | OTLP 플래그 비트 필드.             |

## `sigiro_logs`

| 열                    | 유형                 | 단위         | 의미                               |
| --------------------- | -------------------- | ------------ | ---------------------------------- |
| `timestamp`           | `TIMESTAMP NOT NULL` | µs precision | 이벤트 또는 데이터 포인트 시각.    |
| `observed_timestamp`  | `BIGINT NOT NULL`    | epoch µs     | 로그 관측 시각.                    |
| `trace_id`            | `VARCHAR`            | —            | 추적 ID(16진 문자열).              |
| `span_id`             | `VARCHAR`            | —            | 스팬 ID(16진 문자열).              |
| `event_name`          | `VARCHAR`            | —            | 로그 이벤트 이름.                  |
| `service_name`        | `VARCHAR NOT NULL`   | —            | OTel 리소스의 service.name.        |
| `service_namespace`   | `VARCHAR`            | —            | OTel 리소스의 service.namespace.   |
| `service_instance_id` | `VARCHAR`            | —            | OTel 리소스의 service.instance.id. |
| `severity_number`     | `INTEGER NOT NULL`   | —            | OTLP 심각도 번호.                  |
| `severity_text`       | `VARCHAR NOT NULL`   | —            | 심각도 레이블.                     |
| `body`                | `VARCHAR`            | —            | 로그 본문 문자열.                  |
| `resource_attributes` | `VARCHAR`            | —            | 리소스 속성의 JSON 문자열.         |
| `scope_name`          | `VARCHAR`            | —            | 계측 범위 이름.                    |
| `scope_version`       | `VARCHAR`            | —            | 계측 범위 버전.                    |
| `scope_attributes`    | `VARCHAR`            | —            | 범위 속성의 JSON 문자열.           |
| `log_attributes`      | `VARCHAR`            | —            | 로그 속성의 JSON 문자열.           |
| `template_id`         | `UBIGINT`            | —            | DRAIN 로그 템플릿 해시.            |
| `template_values`     | `VARCHAR`            | —            | 원래 변수 토큰의 JSON 배열 문자열. |
| `template_text`       | `VARCHAR`            | —            | DRAIN 분류 로그 템플릿.            |

## `sigiro_metrics_gauge`

| 열                    | 유형                 | 단위         | 의미                               |
| --------------------- | -------------------- | ------------ | ---------------------------------- |
| `timestamp`           | `TIMESTAMP NOT NULL` | µs precision | 이벤트 또는 데이터 포인트 시각.    |
| `start_timestamp`     | `BIGINT`             | epoch µs     | 메트릭 데이터 포인트 시작 시각.    |
| `metric_name`         | `VARCHAR NOT NULL`   | —            | 메트릭 이름.                       |
| `metric_description`  | `VARCHAR`            | —            | 메트릭 설명.                       |
| `metric_unit`         | `VARCHAR`            | —            | 전송자가 지정한 단위.              |
| `value`               | `DOUBLE NOT NULL`    | metric_unit  | 게이지 데이터 포인트 값.           |
| `service_name`        | `VARCHAR NOT NULL`   | —            | OTel 리소스의 service.name.        |
| `service_namespace`   | `VARCHAR`            | —            | OTel 리소스의 service.namespace.   |
| `service_instance_id` | `VARCHAR`            | —            | OTel 리소스의 service.instance.id. |
| `resource_attributes` | `VARCHAR`            | —            | 리소스 속성의 JSON 문자열.         |
| `scope_name`          | `VARCHAR`            | —            | 계측 범위 이름.                    |
| `scope_version`       | `VARCHAR`            | —            | 계측 범위 버전.                    |
| `scope_attributes`    | `VARCHAR`            | —            | 범위 속성의 JSON 문자열.           |
| `metric_attributes`   | `VARCHAR`            | —            | 데이터 포인트 속성의 JSON 문자열.  |
| `flags`               | `INTEGER`            | —            | OTLP 플래그 비트 필드.             |
| `exemplars_json`      | `VARCHAR`            | —            | 예시값의 JSON 배열 문자열.         |

## `sigiro_metrics_sum`

| 열                        | 유형                 | 단위         | 의미                               |
| ------------------------- | -------------------- | ------------ | ---------------------------------- |
| `timestamp`               | `TIMESTAMP NOT NULL` | µs precision | 이벤트 또는 데이터 포인트 시각.    |
| `start_timestamp`         | `BIGINT`             | epoch µs     | 메트릭 데이터 포인트 시작 시각.    |
| `metric_name`             | `VARCHAR NOT NULL`   | —            | 메트릭 이름.                       |
| `metric_description`      | `VARCHAR`            | —            | 메트릭 설명.                       |
| `metric_unit`             | `VARCHAR`            | —            | 전송자가 지정한 단위.              |
| `value`                   | `DOUBLE NOT NULL`    | metric_unit  | 합계 데이터 포인트 값.             |
| `service_name`            | `VARCHAR NOT NULL`   | —            | OTel 리소스의 service.name.        |
| `service_namespace`       | `VARCHAR`            | —            | OTel 리소스의 service.namespace.   |
| `service_instance_id`     | `VARCHAR`            | —            | OTel 리소스의 service.instance.id. |
| `resource_attributes`     | `VARCHAR`            | —            | 리소스 속성의 JSON 문자열.         |
| `scope_name`              | `VARCHAR`            | —            | 계측 범위 이름.                    |
| `scope_version`           | `VARCHAR`            | —            | 계측 범위 버전.                    |
| `scope_attributes`        | `VARCHAR`            | —            | 범위 속성의 JSON 문자열.           |
| `metric_attributes`       | `VARCHAR`            | —            | 데이터 포인트 속성의 JSON 문자열.  |
| `flags`                   | `INTEGER`            | —            | OTLP 플래그 비트 필드.             |
| `exemplars_json`          | `VARCHAR`            | —            | 예시값의 JSON 배열 문자열.         |
| `aggregation_temporality` | `INTEGER NOT NULL`   | —            | OTLP 집계 시간성 열거값.           |
| `is_monotonic`            | `BOOLEAN NOT NULL`   | —            | 합계가 단조 증가하는지 여부.       |

## `sigiro_metrics_histogram`

| 열                        | 유형                 | 단위         | 의미                                            |
| ------------------------- | -------------------- | ------------ | ----------------------------------------------- |
| `timestamp`               | `TIMESTAMP NOT NULL` | µs precision | 이벤트 또는 데이터 포인트 시각.                 |
| `start_timestamp`         | `BIGINT`             | epoch µs     | 메트릭 데이터 포인트 시작 시각.                 |
| `metric_name`             | `VARCHAR NOT NULL`   | —            | 메트릭 이름.                                    |
| `metric_description`      | `VARCHAR`            | —            | 메트릭 설명.                                    |
| `metric_unit`             | `VARCHAR`            | —            | 전송자가 지정한 단위.                           |
| `count`                   | `BIGINT NOT NULL`    | samples      | 샘플 수.                                        |
| `sum`                     | `DOUBLE`             | metric_unit  | 기록된 값의 합.                                 |
| `min`                     | `DOUBLE`             | metric_unit  | 기록된 값의 최솟값.                             |
| `max`                     | `DOUBLE`             | metric_unit  | 기록된 값의 최댓값.                             |
| `bucket_counts`           | `VARCHAR NOT NULL`   | samples      | 명시적 히스토그램 버킷 개수의 JSON 배열 문자열. |
| `explicit_bounds`         | `VARCHAR NOT NULL`   | metric_unit  | 명시적 히스토그램 경계의 JSON 배열 문자열.      |
| `service_name`            | `VARCHAR NOT NULL`   | —            | OTel 리소스의 service.name.                     |
| `service_namespace`       | `VARCHAR`            | —            | OTel 리소스의 service.namespace.                |
| `service_instance_id`     | `VARCHAR`            | —            | OTel 리소스의 service.instance.id.              |
| `resource_attributes`     | `VARCHAR`            | —            | 리소스 속성의 JSON 문자열.                      |
| `scope_name`              | `VARCHAR`            | —            | 계측 범위 이름.                                 |
| `scope_version`           | `VARCHAR`            | —            | 계측 범위 버전.                                 |
| `scope_attributes`        | `VARCHAR`            | —            | 범위 속성의 JSON 문자열.                        |
| `metric_attributes`       | `VARCHAR`            | —            | 데이터 포인트 속성의 JSON 문자열.               |
| `flags`                   | `INTEGER`            | —            | OTLP 플래그 비트 필드.                          |
| `exemplars_json`          | `VARCHAR`            | —            | 예시값의 JSON 배열 문자열.                      |
| `aggregation_temporality` | `INTEGER NOT NULL`   | —            | OTLP 집계 시간성 열거값.                        |

## `sigiro_metrics_exp_histogram`

| 열                        | 유형                 | 단위         | 의미                               |
| ------------------------- | -------------------- | ------------ | ---------------------------------- |
| `timestamp`               | `TIMESTAMP NOT NULL` | µs precision | 이벤트 또는 데이터 포인트 시각.    |
| `start_timestamp`         | `BIGINT`             | epoch µs     | 메트릭 데이터 포인트 시작 시각.    |
| `metric_name`             | `VARCHAR NOT NULL`   | —            | 메트릭 이름.                       |
| `metric_description`      | `VARCHAR`            | —            | 메트릭 설명.                       |
| `metric_unit`             | `VARCHAR`            | —            | 전송자가 지정한 단위.              |
| `count`                   | `BIGINT NOT NULL`    | samples      | 샘플 수.                           |
| `sum`                     | `DOUBLE`             | metric_unit  | 기록된 값의 합.                    |
| `min`                     | `DOUBLE`             | metric_unit  | 기록된 값의 최솟값.                |
| `max`                     | `DOUBLE`             | metric_unit  | 기록된 값의 최댓값.                |
| `scale`                   | `INTEGER NOT NULL`   | —            | 지수 히스토그램 스케일.            |
| `zero_count`              | `BIGINT NOT NULL`    | samples      | 영 버킷 개수.                      |
| `zero_threshold`          | `DOUBLE`             | metric_unit  | 영 버킷 임계값.                    |
| `positive_offset`         | `INTEGER`            | —            | 양수 버킷 인덱스 오프셋.           |
| `positive_bucket_counts`  | `VARCHAR`            | samples      | 양수 버킷 개수의 JSON 배열 문자열. |
| `negative_offset`         | `INTEGER`            | —            | 음수 버킷 인덱스 오프셋.           |
| `negative_bucket_counts`  | `VARCHAR`            | samples      | 음수 버킷 개수의 JSON 배열 문자열. |
| `service_name`            | `VARCHAR NOT NULL`   | —            | OTel 리소스의 service.name.        |
| `service_namespace`       | `VARCHAR`            | —            | OTel 리소스의 service.namespace.   |
| `service_instance_id`     | `VARCHAR`            | —            | OTel 리소스의 service.instance.id. |
| `resource_attributes`     | `VARCHAR`            | —            | 리소스 속성의 JSON 문자열.         |
| `scope_name`              | `VARCHAR`            | —            | 계측 범위 이름.                    |
| `scope_version`           | `VARCHAR`            | —            | 계측 범위 버전.                    |
| `scope_attributes`        | `VARCHAR`            | —            | 범위 속성의 JSON 문자열.           |
| `metric_attributes`       | `VARCHAR`            | —            | 데이터 포인트 속성의 JSON 문자열.  |
| `flags`                   | `INTEGER`            | —            | OTLP 플래그 비트 필드.             |
| `exemplars_json`          | `VARCHAR`            | —            | 예시값의 JSON 배열 문자열.         |
| `aggregation_temporality` | `INTEGER NOT NULL`   | —            | OTLP 집계 시간성 열거값.           |

## `sigiro_profiles`

| 열                    | 유형                 | 단위         | 의미                            |
| --------------------- | -------------------- | ------------ | ------------------------------- |
| `profile_id`          | `VARCHAR`            | —            | 프로파일 ID.                    |
| `trace_id`            | `VARCHAR`            | —            | 추적 ID(16진 문자열).           |
| `span_id`             | `VARCHAR`            | —            | 스팬 ID(16진 문자열).           |
| `service_name`        | `VARCHAR NOT NULL`   | —            | OTel 리소스의 service.name.     |
| `timestamp`           | `TIMESTAMP NOT NULL` | µs precision | 이벤트 또는 데이터 포인트 시각. |
| `profile_type`        | `VARCHAR`            | —            | 프로파일 샘플 유형.             |
| `stack_trace`         | `VARCHAR[]`          | —            | 순서대로 해석된 스택 프레임.    |
| `value`               | `BIGINT`             | unit         | 이 값 유형의 샘플 값.           |
| `unit`                | `VARCHAR`            | —            | 프로파일 샘플 유형의 단위.      |
| `resource_attributes` | `VARCHAR`            | —            | 리소스 속성의 JSON 문자열.      |
