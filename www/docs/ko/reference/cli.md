---
title: CLI
description: >-
  모든 sigiro 서브커맨드와 그 인자, 그리고 각 서브커맨드가 읽는 환경 변수입니다. sigiro에는 일곱 개의 서브커맨드가 있습니다. 각
  서브커맨드는 들여쓰기된 JSON을 출력합니다.
sidebar:
  order: 2
---
`sigiro`에는 일곱 개의 서브커맨드가 있습니다. 모든 서브커맨드는 성공 시 들여쓰기된
JSON을 출력합니다. 두 번째 출력 형식은 없으며, TTY 감지도 수행하지 않습니다.

전역 플래그 `--key <TOKEN>`은 호스팅 엔드포인트용 베어러 토큰을 설정합니다. 이 플래그는
`SIGIRO_API_KEY`를 재정의합니다. 자체 호스팅 서버는 토큰을 무시합니다.

모든 클라이언트 서브커맨드는 `SIGIRO_ENDPOINT`에 지정된 서버에 접속하며, 기본값은
`http://localhost:9999`입니다.

## serve

OTLP gRPC 서버, OTLP HTTP 서버, 그리고 쿼리 API를 시작합니다.

| 플래그 | 재정의 대상 | 기본값 |
| --- | --- | --- |
| `--data-dir <PATH>` | `SIGIRO_DATA_DIR` | `~/.local/share/sigiro` |
| `--http-port <PORT>` | `SIGIRO_HTTP_PORT` | `9999` |
| `--otlp-http-port <PORT>` | `SIGIRO_OTLP_HTTP_PORT` | `4318` |
| `--otlp-grpc-port <PORT>` | `SIGIRO_GRPC_PORT` | `4317` |

최초 시작 시 쿼리 확장을 데이터 디렉터리로 다운로드합니다. 이 다운로드는
30~90초가 소요되며, 그동안 시작이 차단됩니다.

## diagnose

`sigiro diagnose <SERVICE>`는 하나의 서비스에 대해 하나의 진단 블록을 반환합니다.

| 플래그 | 타입 | 기본값 |
| --- | --- | --- |
| `--from <EPOCH_SECONDS>` | 정수 | 현재 시각으로부터 15분 전 |
| `--to <EPOCH_SECONDS>` | 정수 | 현재 시각 |

`--from`과 `--to`는 에포크 **초** 단위입니다. 이에 대응하는 HTTP 필드 이름은
`from_ts`와 `to_ts`이며, 이들은 에포크 **마이크로초** 단위입니다. 두 HTTP 필드는
모두 선택 사항입니다. 둘 다 생략하면 서버는 최근 15분을 읽으며, 이는 CLI가
사용하는 것과 동일한 구간입니다.

이 명령은 JSON을 출력합니다. 출력을 좁히려면 `jq`로 파이프하십시오. CLI는 자체적으로
어떠한 필터도 적용하지 않으므로, CLI와 API는 동일한 문서를 반환합니다.

## anomalies

`sigiro anomalies`는 감지된 체제 변화(regime shift)를 변화 폭이 큰 순서대로 나열하며,
각 행에는 신호 간 공통 `incident_id`가 포함됩니다.

| 플래그 | 타입 | 기본값 |
| --- | --- | --- |
| `--service <NAME>` | 문자열 | 모든 서비스 |
| `--from <EPOCH_SECONDS>` | 정수 | 하한 없음 |
| `--to <EPOCH_SECONDS>` | 정수 | 상한 없음 |

각 행은 예약된 처리 과정이 기록하는 `sigiro_anomalies` 테이블에서 가져옵니다.
요청 시점에 계산되는 것은 없습니다.

## query

`sigiro query "<SQL>"`는 로컬 클라이언트가 아니라 **연합 코디네이터(federated coordinator)**
입니다. 하나 이상의 엣지를 확인하고, 각 엣지에 부분 쿼리를 푸시한 뒤, 그 부분 결과를
하나의 응답으로 취합합니다.

| 플래그 | 타입 | 기본값 |
| --- | --- | --- |
| `<SQL>` | 문자열 | 없음 |
| `--sql-file <PATH>` | 경로, 또는 표준 입력을 뜻하는 `-` | 없음 |
| `--edge <NAME=HOST:PORT>` | 반복 지정 가능 | 없음 |
| `--full-scan` | 플래그 | 꺼짐 |

엣지 확인은 다음 순서를 따릅니다. `SIGIRO_EDGES`(쉼표로 구분된
`name=host:port`)와 `--edge`는 그대로 사용되며, 이 중 하나라도 지정되면 탐색을 건너뜁니다.
둘 다 설정되지 않은 경우 sigiro는 `tailscale status --json`을 실행하여
`tag:sigiro-edge` 태그가 지정된 모든 피어를 가져옵니다. 확인된 엣지가 하나도 없으면
명령은 대상 없이 실행되는 대신 실패합니다.

따라서 `sigiro query`는 단순한 로컬 서버를 대상으로는 동작하지 **않습니다**. 엣지는
`SIGIRO_QUERY_SURFACE=quack`으로 데이터를 제공해야 하며, `--edge`로 이름을 지정하거나
tailnet에서 찾아야 합니다. 단일 로컬 서버에 대해 SQL을 실행하려면 대신 SQL을
`/v1/query`로 POST하십시오.

`--full-scan`은 `timestamp`에 대한 `WHERE` 제한이 없는 쿼리를 허용합니다. sigiro는
기본적으로 그러한 쿼리를 거부합니다.

## status

`sigiro status`는 서버의 `/health` 엔드포인트를 읽어 헬스 블록을 출력합니다.
`/health`는 키를 필요로 하지 않습니다.

## fetch-extensions

`sigiro fetch-extensions`는 최초 `sigiro serve` 실행 전에 쿼리 확장을 데이터 디렉터리로
다운로드합니다. sigiro를 설치한 후 한 번 실행하거나, 외부 네트워크 접속이 불가능한
호스트로 배포하기 전에 실행하십시오.

## healthcheck

`sigiro healthcheck`는 로컬 `/health` 엔드포인트를 검사하고, 서버가 준비된 경우에만
`0`으로 종료합니다. distroless 이미지에는 셸도 `curl`도 없으므로, 이것이 컨테이너의
`HEALTHCHECK`입니다.
