---
title: sigiro를 Executor에 연결하기
description: sigiro의 OpenAPI API나 문서 MCP 서버를 Executor에 추가한 다음, 그 결과로 생긴 도구를 에이전트에 제공합니다.
sidebar:
  order: 6
---

[Executor](https://executor.sh/)는 에이전트를 위한 통합 계층입니다. OpenAPI 명세, MCP 서버, GraphQL 엔드포인트를 하나의 도구 카탈로그로 가져옵니다. Executor 플러그인을 작성하지 않고도 sigiro를 추가할 수 있습니다.

Executor는 카탈로그를 채우기 위해 [integrations.sh](https://integrations.sh/) 레지스트리도 읽으며, sigiro는 그곳에 자신의 인터페이스를 게시하므로 아무것도 붙여 넣기 전에 Executor 안의 검색으로 sigiro를 찾을 수도 있습니다. 아래 단계는 명시적인 가져오기이며, 어느 경우에나 동작합니다.

## sigiro API 추가하기

공개 OpenAPI 문서는 다음과 같습니다.

```
https://sigiro.com/openapi.json
```

Executor 웹 UI에서:

1. **Add Integration**을 엽니다.
2. OpenAPI URL을 붙여 넣습니다.
3. Executor가 작업을 감지하고 색인하게 합니다.
4. 통합에 대한 연결을 만듭니다.
5. API가 요구하면 그 연결에 호스팅 sigiro OAuth 액세스 토큰을 추가합니다.
6. 도구마다 정책을 설정합니다. SQL을 실행하거나 텔레메트리를 조사하는 도구에는 승인을 사용하십시오.

명세는 공개되어 있어 읽는 데 키가 필요하지 않습니다. 호스팅 API 자체에는 여전히 OAuth 액세스 토큰이 필요합니다. 셀프 호스팅 sigiro도 자신의 인스턴스가 제공하는 OpenAPI URL로 같은 방식으로 가져올 수 있으며, 셀프 호스팅 서버는 오픈 모드로 실행되어 어떤 요청도 인증하지 않으므로 키가 전혀 필요하지 않습니다.

Executor에는 CLI도 있습니다. 같은 가져오기는 다음과 같은 형태입니다.

```bash
executor call executor openapi addIntegration '{
  "spec": "https://sigiro.com/openapi.json",
  "namespace": "sigiro",
  "baseUrl": "https://sigiro.com"
}'
```

명세에 적힌 서버 URL이 Executor가 호출하기를 원하는 URL이 아니라면 `baseUrl` 필드를 사용하십시오. 인증은 공개 명세 URL이 아니라 생성된 연결에 설정하십시오.

활성화되었는지 확인합니다.

```bash
executor tools integrations
```

## sigiro 문서 MCP 서버 추가하기

문서 사이트는 공개 읽기 전용 MCP 서버도 제공합니다.

```
https://sigiro.com/mcp/docs
```

Executor에서 MCP 통합으로 추가하고 streamable HTTP 전송을 선택하십시오. 이렇게 하면 에이전트가 검색과 페이지 조회 같은 문서 도구를 사용할 수 있습니다. 워크스페이스의 텔레메트리에는 접근할 수 없습니다. `/docs`를 뺀 같은 URL인 `https://sigiro.com/mcp`는 텔레메트리 도구를 추가하며 OAuth 로그인이 필요합니다.

실시간 텔레메트리 작업에는 OpenAPI 통합을 사용하십시오. 에이전트가 sigiro 문서를 찾거나 읽어야 할 때는 MCP 통합을 사용하십시오. 둘 다 같은 Executor 워크스페이스에 추가할 수 있습니다.

## 에이전트 연결하기

Executor가 통합을 색인한 뒤, 에이전트를 MCP로 Executor에 연결하십시오. 에이전트가 사용하는 것은 Executor의 MCP 엔드포인트이며, 별도의 sigiro 플러그인은 필요하지 않습니다. 배포에서 사용하는 엔드포인트와 전송은 [Executor의 MCP 연결 가이드](https://executor.sh/docs)를 따르십시오.

## 가져오기 확인하기

에이전트에게 사용하게 하기 전에 Executor에서 통합을 확인하십시오. sigiro API 가져오기는 서비스, 트레이스, 알림, 쿼리에 대한 작업을 제공해야 합니다. 문서 가져오기는 문서 검색과 페이지 조회를 제공해야 합니다.

그런 다음 읽기 전용 호출 하나를 먼저 시험하십시오. 호스팅 API 연결에서는 요청에 OAuth 액세스 토큰이 포함되는지, 반환된 데이터가 의도한 sigiro 테넌트의 것인지 확인하십시오. OAuth 액세스 토큰을 이 페이지, OpenAPI URL, 저장소에 커밋되는 에이전트 프롬프트에 넣지 마십시오.

## 다음 단계

- [API 참조](/docs/reference) — 생성된 작업의 세부 정보를 살펴봅니다
- [호스티드 sigiro로 텔레메트리 전송](/docs/how-to/hosted-onboarding) — 호스팅 엔드포인트와 테넌트 연결을 만듭니다
- [빠른 시작](/docs/tutorials/quickstart) — sigiro를 로컬에서 실행하고 테스트 데이터를 보냅니다
