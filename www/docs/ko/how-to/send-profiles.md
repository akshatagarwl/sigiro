---
title: Pyroscope 프로파일을 sigiro로 전송하기
description: 검증된 Pyroscope Rust SDK에서 pprof 프로파일을 Pyroscope 호환 HTTP로 sigiro에 보내고, 프로파일 행을 SQL로 확인합니다.
sidebar:
  order: 7
---

이 가이드는 애플리케이션 CPU 프로파일을 sigiro로 보내는 방법을 보여 줍니다. 검증된 퍼스트 파티 Pyroscope Rust SDK와 그 pprof 백엔드를 사용합니다.

이는 에이전트 세션을 OpenTelemetry로 기록하는 [에이전트 자체 텔레메트리 전송](/docs/how-to/agent-telemetry)과는 다릅니다. 살펴보려는 프로세스가 pprof 프로파일을 생성할 때 이 가이드를 사용하십시오.

## 시작하기 전에

sigiro 서버가 필요합니다. `sigiro serve`로 시작하거나 [빠른 시작](/docs/tutorials/quickstart#1-실행하기)의 Docker 명령을 사용하십시오.

Pyroscope 호환 HTTP는 sigiro의 메인 포트 `9999`와 OTLP HTTP 포트 `4318`에서 사용할 수 있습니다. 수집 트래픽을 의도적으로 분리하는 경우가 아니라면 메인 포트를 사용하십시오. sigiro는 포트 `4040`에서 프로파일을 받지 않습니다.

Open 인증 모드의 셀프 호스팅 서버에는 키가 필요하지 않습니다. 호스팅 sigiro를 포함한 Better Auth OAuth/JWT 모드의 서버에는 `Authorization: Bearer YOUR_TENANT_KEY`가 필요합니다.

## pyroscope-rs 설정하기

sigiro는 퍼스트 파티 `pyroscope` 크레이트를 pprof 백엔드와 함께 검증합니다. 이 저장소는 현재 버전 `2.0.4`를 고정합니다.

```toml
[dependencies]
pyroscope = { version = "=2.0.4", features = ["backend-pprof-rs"] }
```

셀프 호스팅 sigiro에는 다음 대상을 사용하고 헤더는 넣지 않습니다.

```rust
let endpoint = "http://localhost:9999";
let http_headers = None;
```

호스팅 sigiro에는 포트 없는 HTTPS 오리진을 사용하고 OAuth 액세스 토큰을 추가합니다.

```rust
use std::collections::HashMap;

let endpoint = "https://sigiro.com";
let mut headers = HashMap::new();
headers.insert(
    "Authorization".to_string(),
    format!("Bearer {}", std::env::var("SIGIRO_ACCESS_TOKEN")?),
);
let http_headers = Some(headers);
```

애플리케이션 시작 시 그 대상과 선택적 헤더 맵을 SDK에 전달합니다.

```rust
use pyroscope::backend::pprof::PprofConfig;
use pyroscope::backend::{BackendConfig, pprof_backend};
use pyroscope::pyroscope::PyroscopeAgentBuilder;

let backend = pprof_backend(PprofConfig::default(), BackendConfig::default());
let mut builder = PyroscopeAgentBuilder::new(
    endpoint,
    "checkout",
    100,
    "pyroscope-rs",
    env!("CARGO_PKG_VERSION"),
    backend,
);
if let Some(headers) = http_headers {
    builder = builder.http_headers(headers);
}
let _profile_agent = builder.build()?.start()?;
```

`_profile_agent`는 프로세스가 실행되는 동안 계속 스코프 안에 두십시오. SDK는 10초마다 배치를 보냅니다. 핸들이 일찍 해제되면 이후의 배치는 sigiro에 도달하지 않습니다.

OAuth 액세스 토큰을 소스 관리에 넣지 마십시오. 프로세스를 시작하는 환경에서 `SIGIRO_ACCESS_TOKEN`을 설정하십시오.

## HTTP 계약

SDK는 다음의 동등한 경로 중 어디로든 보낼 수 있습니다.

- `/`
- `/push.v1.PusherService/Push`
- `/ingest`
- `/ingest/pyroscope`

Rust SDK는 위 대상에 `/push.v1.PusherService/Push`를 덧붙입니다. 따라서 이미 그 경로가 포함된 대상은 sigiro에서 뚜렷한 오류 없이 잘못된 URL로 보낼 수 있습니다. 이 SDK에는 오리진만 주십시오.

sigiro는 다음 요청 형식을 받습니다.

- gzip으로 압축한 Pyroscope `PushRequest`에는 `application/proto`, `binary/octet-stream` 또는 콘텐츠 타입 없음
- 원시 pprof 바이트나 `PushRequest`에는 이름이 정확히 `profile`인 파일 필드를 가진 `multipart/form-data`

`name` 쿼리 매개변수는 sigiro가 본문을 디코딩하는 방식을 바꿉니다. `name`이 있으면 sigiro는 `profile` 바이트를 원시 pprof로 취급하고, 첫 `{` 앞의 텍스트를 서비스 이름으로 사용합니다. `name`이 없으면 본문은 `PushRequest`로 디코딩되어야 하며, 각 시리즈에는 비어 있지 않은 `service_name` 레이블이 있어야 합니다. `name` 없이 원시 pprof를 보내거나 `service_name` 없는 `PushRequest`를 보내면 `400`이 반환되고 아무것도 저장되지 않습니다.

sigiro는 gzip을 받으며, 기본적으로 각 요청을 압축 해제 후 8 MiB로 제한합니다. HTTP 본문 제한을 넘는 요청은 `413`을 반환합니다. 잘못되었거나 지원되지 않는 프로파일은 `400`, Better Auth OAuth/JWT 베어러 토큰이 없거나 유효하지 않으면 `401`, 용량 게이트에 걸리면 `507`을 반환합니다. `200` 응답은 sigiro가 배치를 받았다는 뜻입니다.

pprof 데이터만 보내십시오. sigiro는 Java Flight Recorder 데이터를 디코딩하지 않으므로, 이 가이드는 Java나 JFR 호환성을 주장하지 않습니다. 다른 언어 SDK 설정은 실행 가능한 엔드 투 엔드 검증이 생길 때까지 생략합니다.

## 동작 확인하기

애플리케이션을 시작하고 최소 10초 동안 작업을 하게 하십시오. 그런 다음 메인 HTTP API에서 프로파일 행을 다시 읽습니다.

```bash
curl -s -X POST http://localhost:9999/v1/query \
  --data "SELECT service_name, profile_type, unit, count(*) AS samples
          FROM sigiro_profiles
          WHERE service_name = 'checkout'
            AND timestamp > now() - INTERVAL '1 hour'
          GROUP BY 1, 2, 3
          ORDER BY samples DESC"
```

호스팅 sigiro에서는 오리진을 바꾸고 `-H "Authorization: Bearer YOUR_TENANT_KEY"`를 추가하십시오. 결과가 비어 있지 않으면 SDK 전송, 프로파일 디코딩, HTTP 재조회가 확인된 것입니다. SDK 주기가 한 번 지난 뒤에도 결과가 비어 있다면, 대개 프로파일 핸들이 해제되었거나, 대상에 불필요한 경로가 포함되었거나, 쿼리의 서비스 이름이 SDK 설정과 일치하지 않는 경우입니다.

## 다음 단계

- [호스티드 sigiro로 텔레메트리 전송](/docs/how-to/hosted-onboarding) — 호스팅 인증과 다른 텔레메트리 신호를 설정합니다
- [테이블 이해하기](/docs/explanation/tables) — 각 테이블에 어떤 텔레메트리가 저장되는지 확인합니다
