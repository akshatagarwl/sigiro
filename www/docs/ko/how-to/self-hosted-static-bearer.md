---
title: 정적 베어러 토큰으로 자체 호스팅 보호하기
description: 쿼리 API와 OTLP HTTP·gRPC 리스너에 하나의 비밀 토큰을 적용합니다.
sidebar:
  order: 8
---

로컬 자체 호스팅 서버에서 `SIGIRO_STATIC_BEARER_TOKEN`을 설정하면 쿼리 HTTP, OTLP HTTP, OTLP gRPC 리스너 모두에 같은 토큰이 필요합니다. `openssl rand -hex 32`로 무작위 값을 생성하고 비밀 관리 시스템을 통해 서버에 전달하십시오. 토큰에는 32바이트 이상의 ASCII bearer 토큰 문자만 사용할 수 있습니다. 빈 값, 짧은 값, 잘못된 문자, UTF-8이 아닌 값은 시작 시 거부됩니다. 변수를 설정하지 않으면 로컬 서버는 요청을 인증하지 않는 Open 모드로 동작합니다. Open 모드 리스너를 신뢰할 수 없는 네트워크에 노출하지 마십시오.

CLI와 stdio MCP 클라이언트에는 동일한 값을 `SIGIRO_ACCESS_TOKEN` 또는 `--token`으로 전달합니다. OTLP 익스포터와 HTTP 클라이언트는 `Authorization` 헤더에 `Bearer ` 접두사와 토큰을 보냅니다. 이 모드는 정확히 `Bearer` 스킴만 허용하며 `AgentAuth`는 거부합니다. 인증된 요청은 모두 로컬 `default` 테넌트에 속합니다. `/health`는 공개됩니다. `/admin/*`는 별도의 `SIGIRO_ADMIN_TOKEN`으로 보호하며 정적 베어러 토큰은 관리자 자격 증명이 아닙니다.

이 모드는 **로컬 스토리지 자체 호스팅 전용**입니다. `SIGIRO_AUTH_ISSUER`, `SIGIRO_AUTH_AUDIENCE`, `SIGIRO_AUTH_JWKS_URL`, 분산 PostgreSQL 카탈로그 또는 `SIGIRO_QUERY_SURFACE=quack`과 함께 설정하면 시작에 실패합니다. 기본 HTTP 쿼리 리스너에는 기본 TLS가 적용되지 않으므로 원격 액세스에는 TLS 종료 리버스 프록시를 사용하십시오. OTLP HTTP·gRPC의 기본 TLS에는 `SIGIRO_TLS_CERT`와 `SIGIRO_TLS_KEY`를 설정하거나 신뢰할 수 있는 프록시에서 TLS를 종료합니다. 평문 연결로 토큰을 전송하지 마십시오. 토큰을 교체하려면 클라이언트를 갱신하고 서버를 재시작해야 합니다. 두 토큰을 동시에 허용하는 교체 기간이나 수집과 쿼리의 별도 권한은 없습니다.
