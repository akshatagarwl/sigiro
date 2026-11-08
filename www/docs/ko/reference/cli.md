---
title: CLI
description: 호스팅 가입과 로그인을 포함한 sigiro의 모든 하위 명령, 인수, 그리고 읽는 환경 변수.
sidebar:
  order: 2
---

`sigiro`에는 여덟 개의 하위 명령이 있습니다. 클라이언트 명령은 성공하면 들여쓰기된 JSON을
출력하고, 서버 명령은 해당 프로토콜이나 리스너에 연결된 채로 계속 실행됩니다.

새 호스팅 계정을 만들려면 `sigiro signup --name "Your Name" --email
you@example.com`을 실행합니다. 여섯 자리 OTP를 이메일로 보내고, 터미널에서 안전하게 입력받고,
개인 워크스페이스를 정확히 하나 만들어 활성화하고, OAuth 디바이스 인증을 내부에서 완료한 다음,
리프레시/액세스 자격 증명을 OS 자격 증명 저장소에 저장합니다. 기존 계정에는 브라우저 기반
`sigiro auth login`을 사용하십시오. `--no-browser`를 주면 대신 확인 URL을 출력합니다.
`sigiro auth status`, `sigiro auth token`, `sigiro auth logout`으로 저장된 세션을 관리합니다.

전역 플래그 `--token <TOKEN>`은 자동화를 위한 명시적 재정의입니다. `SIGIRO_ACCESS_TOKEN`보다
우선합니다. 오픈 모드의 셀프 호스팅 서버에는 필요하지 않고, 정적 베어러 인증을 설정한 셀프 호스팅
서버에는 필요합니다.

모든 클라이언트 하위 명령(`status`, `mcp`)은 다음 순서로 서버를 고릅니다.

1. 설정되어 있으면 `SIGIRO_ENDPOINT`.
2. `sigiro auth login`이나 `sigiro signup`이 세션을 저장했으면 `https://sigiro.com`.
3. 그 밖에는 `http://localhost:9999`.

따라서 호스팅 sigiro에 로그인한 뒤에는 로컬에서 `sigiro serve`가 실행 중이어도 이 명령들은
`sigiro.com`과 통신합니다. 로컬 서버에 연결하려면 `SIGIRO_ENDPOINT=http://localhost:9999`를
설정하거나 `sigiro auth logout`을 실행하십시오.

## 호스팅 에이전트 인증

sigiro는 내부에서 공식 Agent Auth 구현을 사용하며, 온보딩용 두 번째 CLI는 제공하지 않습니다.
위임 호스트 등록, 일회성 승인, 최소 권한 capability 부여, 토큰 사용, 키 교체, 폐기는
[Agent Auth로 에이전트 온보딩하기](/docs/how-to/agent-auth)를 참조하십시오.

## ingest-keys

호스팅 sigiro에서 오래 실행되는 서비스를 위한 고정 키로, 15분짜리 `sigiro auth token` 대신 사용합니다. 키는 로그인이 연결된 워크스페이스에 속하고, `Authorization: Bearer <key>`로 전송되며, 텔레메트리 전송만 할 수 있습니다.

| 명령                                      | 하는 일                                                |
| ----------------------------------------- | ------------------------------------------------------ |
| `sigiro ingest-keys create --name <NAME>` | 키를 만들고 한 번만 출력합니다. 해시만 보관합니다      |
| `sigiro ingest-keys list`                 | ID, 이름, 마지막 사용, 취소 여부를 나열합니다. 비밀은 없습니다 |
| `sigiro ingest-keys revoke <ID>`          | 키를 취소합니다. 그 뒤로 송신자는 `401`을 받습니다     |

키는 자신의 로그인으로만 관리할 수 있습니다. Agent Auth 토큰이나 수집 키로는 관리할 수 없습니다.

## serve

OTLP gRPC 서버, OTLP HTTP 서버, 쿼리 API를 시작합니다.

| 플래그                    | 재정의하는 변수         | 기본값                  |
| ------------------------- | ----------------------- | ----------------------- |
| `--data-dir <PATH>`       | `SIGIRO_DATA_DIR`       | `~/.local/share/sigiro` |
| `--http-port <PORT>`      | `SIGIRO_HTTP_PORT`      | `9999`                  |
| `--otlp-http-port <PORT>` | `SIGIRO_OTLP_HTTP_PORT` | `4318`                  |
| `--otlp-grpc-port <PORT>` | `SIGIRO_GRPC_PORT`      | `4317`                  |

처음 시작할 때 쿼리 확장을 데이터 디렉터리로 내려받습니다. 이 다운로드는 30–90초가 걸리며,
그동안 시작이 멈춥니다.

인증이 없는 서버(셀프 호스팅 기본값)는 웹 대시보드도
`http://localhost:9999/dashboard/`에서 제공합니다. 대시보드는 바이너리에 내장되어
있습니다. 다른 정적 빌드를 제공하려면 그 빌드가 있는 디렉터리(`dashboard/dist`)를
`SIGIRO_DASHBOARD_DIR`로 지정하십시오.

## query

`sigiro query "<SQL>"`은 로컬 클라이언트가 아니라 **페더레이션 코디네이터**입니다. 하나 이상의
엣지를 확인하고, 각 엣지에 부분 쿼리를 보내고, 부분 결과를 하나의 답으로 합칩니다.

| 플래그                    | 타입        | 기본값 |
| ------------------------- | ----------- | ------ |
| `<SQL>`                   | 문자열      | 없음   |
| `--edge <NAME=HOST:PORT>` | 반복 가능   | 없음   |
| `--full-scan`             | 플래그      | 꺼짐   |

엣지는 다음 순서로 확인합니다. `SIGIRO_EDGES`(쉼표로 구분한 `name=host:port`)와 `--edge`는
그대로 사용되며, 둘 중 하나라도 있으면 탐색을 건너뜁니다. 둘 다 없으면 sigiro는
`tailscale status --json`을 실행하고 `tag:sigiro-edge` 태그가 붙은 모든 피어를 사용합니다.
확인된 엣지가 하나도 없으면 아무것도 없는 대상에 실행하지 않고 명령이 실패합니다.

따라서 `sigiro query`는 호스팅 `sigiro.com`이나 일반 로컬 서버에서는 **동작하지 않습니다**.
엣지는 `SIGIRO_QUERY_SURFACE=quack`으로 데이터를 제공해야 하고, `--edge`로 지정하거나
tailnet에서 찾을 수 있어야 합니다. 호스팅 sigiro나 로컬 서버 하나에 SQL을 실행하려면
[`sigiro mcp`](#mcp)의 `run_query` 도구를 사용하거나 SQL을 `/v1/query`로 POST하십시오.

```bash
curl -s https://sigiro.com/v1/query \
  -H "Authorization: Bearer $(sigiro auth token)" \
  --data "SELECT service_name, count(*) AS spans FROM sigiro_spans
          WHERE timestamp > now() - INTERVAL '1 hour' GROUP BY 1"
```

`--full-scan`은 `timestamp`에 대한 `WHERE` 범위가 없는 쿼리를 허용합니다. 기본적으로
sigiro는 그런 쿼리를 거부합니다.

## status

`sigiro status`는 위 방식으로 고른 서버의 `/health` 엔드포인트를 읽고, 상태 블록과 함께 최근
5분간의 스팬 수와 로그 수가 담긴 `recent_ingest` 블록을 출력합니다. `endpoint` 필드는 확인한
서버를 나타냅니다. `/health`에는 키가 필요하지 않습니다.

## healthcheck

`sigiro healthcheck`는 로컬 `/health` 엔드포인트를 확인하고, 서버가 준비되었을 때만 `0`으로
종료합니다. distroless 이미지에는 셸도 `curl`도 없으므로 이것이 컨테이너 `HEALTHCHECK`입니다.

## mcp

`sigiro mcp`는 stdio Model Context Protocol 서버를 시작합니다. `list_services`와 `run_query`,
그리고 문서 도구(`search_docs`, `get_page`, `list_pages`, `get_navigation`)를 제공하므로,
분석 에이전트가 HTTP 요청을 직접 만들지 않고 sigiro를 살펴볼 수 있습니다. 위 방식으로 고른
서버를 사용하므로 `sigiro auth login` 뒤에는 저장된 세션으로 호스팅 워크스페이스를 읽고,
세션도 스스로 갱신합니다.

에이전트에 stdio 서버로 등록하십시오.

```bash
claude mcp add sigiro -- sigiro mcp     # Claude Code
codex mcp add sigiro -- sigiro mcp      # Codex
```

에이전트가 셸보다 좁은 `PATH`로 시작한다면 `sigiro`의 절대 경로를 사용하십시오. `run_query`에서는
`timestamp > now() - INTERVAL '1 hour'`처럼 타임스탬프 값으로 `timestamp`의 범위를 정하십시오.
결과 행은 타임스탬프와 `duration`을 Unix epoch 이후의 마이크로초로 표시하지만, `timestamp`를
정수와 비교하면 실패합니다.

호스팅 sigiro는 CLI 세션 대신 브라우저에서 OAuth로 로그인하는 에이전트를 위해 같은 도구를
streamable HTTP로 `https://sigiro.com/mcp`에서도 제공합니다. 로그인 없이 쓸 수 있는 문서 도구만은
`https://sigiro.com/mcp/docs`에 있습니다.
