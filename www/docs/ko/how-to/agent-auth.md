---
title: Agent Auth로 에이전트 온보딩하기
description: Sigiro Agent Auth API를 사용하여 에이전트 호스트를 등록하고, 최소 capability를 부여하며, 자격 증명을 교체하거나 폐기합니다.
sidebar:
  order: 2
---

사람이 아니라 소프트웨어가 호스팅 Sigiro에 인증해야 할 때 Agent Auth를 사용합니다.
Sigiro는 위임 ID를 지원합니다. 사용자가 에이전트를 한 번 승인하여 워크스페이스에 연결하면,
에이전트는 자체 키와 단기 토큰으로 무인 실행할 수 있습니다.

별도의 Sigiro 실행 파일은 필요하지 않습니다. 공식 `@auth/agent` SDK를 에이전트 호스트에
통합하십시오. Sigiro는 자율 에이전트 ID를 지원하지 않습니다.

## 시작하기 전에

먼저 호스팅 Sigiro 계정을 만들거나 로그인하십시오.

```bash
sigiro signup --name "Your Name" --email you@example.com
# 기존 계정:
sigiro auth login
```

계정은 정확히 하나의 Sigiro 워크스페이스에 속해야 합니다. Agent Auth는 승인된 에이전트를
해당 워크스페이스에 연결합니다.

SDK에는 영구 비밀 저장소를 사용하십시오. 호스트와 에이전트의 Ed25519 개인 키와 연결 레코드를
저장합니다. 기본 메모리 저장소는 단기 예시에만 적합하며, 이를 잃으면 새 ID를 등록해야 합니다.

## 1. Sigiro 검색하기

수명 주기 경로를 하드 코딩하지 말고 실제 공급자 문서를 확인하십시오.

```bash
curl -fsS https://sigiro.com/.well-known/agent-configuration | jq
```

issuer가 `https://sigiro.com/api/auth`이고, 보호 리소스가 `https://sigiro.com`이며,
`modes`에는 `delegated`만 포함되는지 확인하십시오.

사용 가능한 capability를 나열합니다.

```bash
curl -fsS https://sigiro.com/api/auth/capability/list | jq
```

프로세스에 필요한 것만 요청하십시오. OTLP 내보내기에는 `telemetry:ingest`만 필요하며,
읽기 또는 알림 관리 capability는 필요하지 않습니다.

## 2. 공식 SDK 추가하기

```bash
aube add @auth/agent@0.6.2
```

Sigiro 서비스 origin으로 클라이언트를 만드십시오. 프로덕션에서는 비밀 저장소가 지원하는
`KVStorage`와 같은 영구 `storage`를 전달해야 합니다. 개인 키를 소스 제어, 로그, 스크린샷,
암호화되지 않은 애플리케이션 상태에 저장하지 마십시오.

```ts
import { AgentAuthClient, MemoryStorage } from '@auth/agent';

const client = new AgentAuthClient({
  urls: ['https://sigiro.com'],
  allowDirectDiscovery: true,
  hostName: 'production-telemetry-host',
  storage: new MemoryStorage(), // 프로덕션에서는 영구 비밀 저장소로 교체하십시오.
  onApprovalRequired: (approval) => {
    const url = approval.verification_uri_complete ?? approval.verification_uri;
    approvalUi.show({ url, code: approval.user_code });
  },
});

const providers = await client.init();
if (providers.length !== 1) throw new Error('Sigiro 검색 실패');
```

`approvalUi.show`는 인증되었으며 내용을 영구 저장하지 않는 운영자 UI를 나타냅니다. 승인 URL이나
코드를 애플리케이션 로그에 기록해서는 안 됩니다.

`connectAgent`는 호스트와 에이전트 Ed25519 키를 생성합니다. Sigiro 검색 문서와 프로덕션
구성은 동적 호스트 등록을 허용하므로 이 경로에는 별도의 호스트 등록 명령이나 토큰이 필요하지
않습니다. 관리자가 미리 프로비저닝한 호스트는 일회성 등록 토큰과 함께 SDK의 `enrollHost`
흐름을 사용합니다. 두 경로를 혼용하지 마십시오.

## 3. 에이전트 등록 및 승인하기

위임 모드로 연결하고 텔레메트리 수집만 요청하십시오.

```ts
const connection = await client.connectAgent({
  provider: 'https://sigiro.com',
  mode: 'delegated',
  name: 'production telemetry exporter',
  capabilities: ['telemetry:ingest'],
  reason: "Send this service's OpenTelemetry data to Sigiro",
  preferredMethod: 'device_authorization',
});

if (connection.status !== 'active') {
  throw new Error(`에이전트가 활성 상태가 아닙니다: ${connection.status}`);
}
```

표시된 확인 URL을 열고 Sigiro에 로그인한 다음 코드를 확인하고 요청된 capability를
승인하십시오. SDK는 승인 성공 또는 요청 만료까지 폴링합니다. 승인 링크와 코드는 일시적이지만
로그나 지원 티켓에 기록하지 마십시오.

승인 후에는 `connection.agentId`를 SDK의 영구 상태와 함께 보관하십시오. 개인 키를 내보내거나
복사하지 마십시오.

## 4. 텔레메트리 인증하기

텔레메트리 요청 직전에 capability 범위가 지정된 새 토큰에 서명하십시오.

```ts
const authorization = await client.signJwt({
  agentId: connection.agentId,
  audience: 'https://sigiro.com',
  capabilities: ['telemetry:ingest'],
});

const headers = {
  Authorization: `Bearer ${authorization.token}`,
};
```

이 헤더를 `https://sigiro.com/v1/traces`, `/v1/metrics`, `/v1/logs` 또는
`/v1development/profiles`에 보내는 OTLP/HTTP 요청에 첨부하십시오. 에이전트 토큰은
수명이 짧습니다. 내보내기 전송 계층은 만료 전에 새 토큰을 받아야 하며, 하나의 토큰을 장기
환경 변수에 넣지 마십시오.

Sigiro는 텔레메트리를 수락하기 전에 에이전트 상태, 워크스페이스 연결, audience, 재생 상태,
`telemetry:ingest` 부여를 확인합니다. 서명된 토큰을 다른 요청에 재사용하지 마십시오.

## 5. 에이전트 키 교체하기

SDK를 통해 키를 교체하십시오.

```ts
await client.rotateAgentKey(connection.agentId);
```

등록된 공개 키가 교체되고 구성된 저장소의 개인 키가 갱신됩니다. 교체 후 이전 키로 서명된
토큰을 폐기하십시오. 호스트가 에이전트를 계속 관리할 수 있도록 재시작 후에도 SDK 저장소를
사용할 수 있어야 합니다.

## 6. 폐기하고 확인하기

음성 테스트에만 사용할 토큰을 만든 다음 에이전트를 폐기하십시오.

```ts
const tokenBeforeRevocation = await client.signJwt({
  agentId: connection.agentId,
  audience: 'https://sigiro.com',
  capabilities: ['telemetry:ingest'],
});

await client.disconnectAgent(connection.agentId);
```

`disconnectAgent`는 로컬 연결을 제거하기 전에 원격 ID와 capability 부여를 폐기합니다.
이후 `tokenBeforeRevocation`을 사용한 요청은 실패해야 합니다. Sigiro OTLP/HTTP 엔드포인트는
폐기된 에이전트 토큰에 HTTP `401`을 반환합니다. 요청이 수락되면 폐기 실패로 처리하고
내보내기를 중지하십시오.

단일 에이전트가 아니라 전체 호스트를 폐기하려면 이를 승인한 Sigiro 계정에서 호스트를
폐기하십시오. 호스트 폐기는 해당 호스트에 연결된 모든 에이전트도 폐기합니다.

## 문제 해결

- **검색 실패:** 공급자 URL로 issuer 경로가 아닌 `https://sigiro.com`을 사용하고,
  well-known 문서를 `curl`로 확인하십시오.
- **승인 만료:** `connectAgent`를 다시 호출하고 새 요청의 제한 시간 안에 승인하십시오.
- **capability 누락:** `telemetry:ingest`를 명시적으로 요청하십시오. 관련 없는 capability를
  추가하여 우회하지 마십시오.
- **재시작 후 ID가 사라짐:** `MemoryStorage`를 영구 `KVStorage` 백엔드로 교체하고 해당
  백엔드를 비밀 자료로 보호하십시오.
- **서명 토큰이 재생으로 거부됨:** 요청마다 새 토큰에 서명하십시오. Sigiro 인증 인스턴스는
  재생 방지를 공유합니다.
- **폐기된 에이전트가 데이터를 계속 보냄:** 프로세스를 중지하고 실패 응답을 보존한 뒤
  Sigiro에 문의하십시오. 원인이 확인될 때까지 대체 ID를 만들지 마십시오.

사람의 가입, OAuth 로그인, 일반 액세스 토큰은
[호스팅 Sigiro로 텔레메트리 보내기](/docs/how-to/hosted-onboarding)를 참조하십시오.
Sigiro CLI 동작은 [CLI 레퍼런스](/docs/reference/cli)를 참조하십시오.
