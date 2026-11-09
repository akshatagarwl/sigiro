---
title: sigiro 문서
description: sigiro는 OpenTelemetry 트레이스, 로그, 메트릭, 프로파일을 저장하고 그에 대한 SQL에 답합니다. 에이전트를 우선으로 설계했으며, 호스팅 워크스페이스에는 사람을 위한 대시보드가 있습니다.
sidebar:
  order: 0
---

sigiro는 OpenTelemetry 트레이스, 로그, 메트릭, 프로파일을 저장하고 그에 대한 SQL에 답합니다.
에이전트를 우선으로 설계했습니다. 모든 답은 에이전트나 사람이 다시 실행할 수 있는 쿼리입니다.
호스팅 워크스페이스에는 사람을 위한 읽기 전용 [대시보드](https://sigiro.com/dashboard/)도 있습니다.

처음이고 자신의 앱 데이터를 sigiro.com으로 보내고 싶다면 [호스티드 sigiro로 텔레메트리
전송](/docs/how-to/hosted-onboarding)부터 시작하십시오. 대신 자신의 머신에서 실행하고 싶다면
[빠른 시작](/docs/tutorials/quickstart)부터 시작하십시오.

이 문서는 네 부분으로 구성됩니다. 각 부분은 서로 다른 종류의 질문에 답하므로,
지금 가진 질문에 맞는 부분으로 이동하십시오.

## 튜토리얼

학습 과정입니다. sigiro에 대한 지식을 전제하지 않으므로 이 부분을 먼저 읽으십시오.
각 단계를 따라가면 단계마다 결과를 확인할 수 있습니다.

- **[빠른 시작](/docs/tutorials/quickstart)** — 자신의 머신에서 서버를 실행하고, 텔레메트리를
  보내고, 최근 실패를 SQL로 조회합니다. 5분이 걸립니다.

## 사용 가이드

하나의 작업을 위한 안내입니다. 원하는 것이 분명하고 그것을 얻기 위한 단계가 필요할 때
이 부분을 읽으십시오.

- **[에이전트로 코드 계측하기](/docs/how-to/install-with-ai)** — 에이전트에게 프롬프트 하나를
  줍니다. 에이전트가 서비스에 OpenTelemetry를 연결하고 sigiro로 향하게 합니다.
- **[Agent Auth로 에이전트 온보딩하기](/docs/how-to/agent-auth)** — 위임된 머신 ID를
  등록하고, 필요한 capability만 승인한 다음, 키를 교체하거나 폐기합니다.
- **[호스티드 sigiro로 텔레메트리 전송](/docs/how-to/hosted-onboarding)** — 웹이나
  `sigiro signup`으로 가입하고, 앱에서 저희가 운영하는 호스트로 OTLP를 보내고, 대시보드에서
  확인합니다.
- **[에이전트 자체 텔레메트리를 sigiro로 전송하기](/docs/how-to/agent-telemetry)** —
  Claude Code, Codex, Hermes Agent, Pi가 sigiro로 보내도록 설정합니다.
- **[에이전트 디버깅 및 최적화](/docs/how-to/debug-agent-performance)** — 세션이 시간과
  토큰을 어디에 썼는지 찾고, 낭비되는 작업을 없애고, 다음 실행을 근거와 함께 비교합니다.
- **[sigiro를 Executor에 연결하기](/docs/how-to/executor)** — OpenAPI API나 문서 MCP 서버를
  에이전트 게이트웨이로 가져옵니다.
- **[프로파일을 sigiro로 전송하기](/docs/how-to/send-profiles)** — Go, OpenTelemetry eBPF
  프로파일러 또는 Collector에서 OTLP 프로파일을 보낸 다음, 프로파일 행을 SQL로 다시 읽습니다.

## 참조

머신에 관한 사실입니다. 작업하면서 이름, 타입, 제한을 확인할 때 이 부분을 읽으십시오.

- **[API 참조](/docs/reference)** — 서버의 `/openapi.json`이 현재 설명하는 HTTP 작업입니다.
  그 문서에 없는 OTLP 수집 경로는 사용 가이드에서 설명합니다.
- **[CLI 참조](/docs/reference/cli)** — 일곱 개의 하위 명령, 모든 플래그, 그리고 이들이 읽는
  모든 환경 변수입니다.

`/openapi.json`을 읽는 에이전트에게는 저희의 다른 정보가 필요하지 않습니다.

## 설명

sigiro가 이렇게 동작하는 이유입니다. 이유, 비용, 대안을 알고 싶을 때 이 부분을 읽으십시오.
터미널은 필요하지 않습니다.

- **[테이블, 그리고 테이블이 설명하는 머신](/docs/explanation/tables)** — 저장되는 신호와,
  다른 머신의 데이터로 답하지 않는 방법입니다.
- **[폐기된 이상 탐지 및 진단 설계](/docs/explanation/evidence)** — 기록용입니다. 순위가
  매겨진 결과 설계는 제거되었습니다. 대신 SQL을 사용하십시오.
- **[폐기된 배포 진단 설계](/docs/explanation/deploys)** — 기록용입니다. 자동 배포 감지는
  제거되었습니다. `service.version`은 여전히 쿼리할 수 있습니다.
