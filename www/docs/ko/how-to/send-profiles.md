---
title: 프로파일을 sigiro로 전송하기
description: Go용 otelprofile, OpenTelemetry eBPF 프로파일러 또는 OpenTelemetry Collector에서 OTLP로 연속 프로파일을 sigiro에 보낸 다음, 프로파일 행을 SQL로 확인합니다.
sidebar:
  order: 7
---

이 가이드는 연속 프로파일(CPU, 힙 및 기타 pprof 프로파일 유형)을 sigiro로 보내는 방법을 보여 줍니다. sigiro가 받는 프로파일 형식은 하나뿐입니다. OpenTelemetry 프로파일 신호인, HTTP 또는 gRPC를 통한 OTLP 프로파일입니다.

이는 에이전트 세션을 OpenTelemetry로 기록하는 [에이전트 자체 텔레메트리 전송](/docs/how-to/agent-telemetry)과는 다릅니다. 프로세스가 CPU 시간과 메모리를 어디에 쓰는지 보고 싶을 때 이 가이드를 사용하십시오.

## 시작하기 전에

sigiro 서버가 필요합니다. `sigiro serve`로 시작하거나 [빠른 시작](/docs/tutorials/quickstart#1-실행하기)의 Docker 명령을 사용하십시오.

셀프 호스팅 서버는 OTLP/HTTP를 포트 `4318`과 메인 포트 `9999`에서, OTLP/gRPC를 포트 `4317`에서 받습니다. 호스티드 sigiro는 OTLP/HTTP만, 포트 없이 `https://sigiro.com`에서 받습니다.

Open 인증 모드의 셀프 호스팅 서버에는 키가 필요하지 않습니다. 호스티드 sigiro를 포함해 Better Auth OAuth/JWT 모드의 서버에는 `Authorization: Bearer YOUR_TENANT_KEY`가 필요합니다.

OpenTelemetry 프로파일 신호는 업스트림에서 아직 개발 중이며, 와이어 형식이 릴리스마다 바뀌어 왔습니다. sigiro는 `opentelemetry-proto` v1.10의 레이아웃을 읽습니다. 다른 리비전으로 만든 생산자가 보낸 프로파일은 sigiro가 `400`으로 거부할 수 있습니다.

## Go: otelprofile

[otelprofile](https://github.com/akshatagarwl/otelprofile)은 Go 자체의 런타임 프로파일러로 Go 프로그램을 안에서 프로파일링하고, OTLP 프로파일로 내보냅니다. 모듈에 추가합니다.

```bash
go get github.com/akshatagarwl/otelprofile@v0.2.0
```

프로그램이 시작될 때 한 번 시작하고, 종료할 때 멈춥니다.

```go
p, err := otelprofile.Start(otelprofile.Config{
    ServiceName: "checkout",
    Endpoint:    "http://localhost:4318",
})
if err != nil {
    log.Fatal(err)
}
defer p.Stop()
```

기본적으로 힙 프로파일은 15초마다, CPU 프로파일은 30초마다 보냅니다. goroutine, mutex, block 프로파일도 보내려면 `ProfileTypes`(또는 `OTEL_GO_X_PROFILES_TYPES=all`)를 설정하십시오.

호스티드 sigiro에서는 엔드포인트를 `https://sigiro.com`으로 하고 키를 추가합니다.

```go
p, err := otelprofile.Start(otelprofile.Config{
    ServiceName: "checkout",
    Endpoint:    "https://sigiro.com",
    Headers: map[string]string{
        "Authorization": "Bearer " + os.Getenv("SIGIRO_ACCESS_TOKEN"),
    },
})
```

해당 필드를 비워 두면 표준 `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_HEADERS`, `OTEL_SERVICE_NAME` 환경 변수도 쓸 수 있습니다. 액세스 토큰을 소스 관리에 넣지 마십시오.

프로그램이 OpenTelemetry Go SDK로 트레이스도 보낸다면, otelprofile은 CPU 샘플에 그것이 실행된 스팬의 레이블을 붙이고, 대시보드는 트레이스 보기에서 스팬의 프로파일을 엽니다. 설정 방법은 otelprofile README를 참조하십시오.

## 모든 Linux 프로세스: OpenTelemetry eBPF 프로파일러

[OpenTelemetry eBPF 프로파일러](https://github.com/open-telemetry/opentelemetry-ebpf-profiler)는 프로그램을 바꾸지 않고 Linux 호스트의 모든 프로세스의 CPU를 샘플링합니다. Collector 배포판 `otelcol-ebpf-profiler` 안에서 `profiling` 리시버로 동작하며, root 권한(또는 `CAP_SYS_ADMIN`, `CAP_PERFMON`, `CAP_BPF`)과 `/proc` 접근이 필요합니다.

이 프로파일러는 각 프로세스에 `process.executable.name`을 붙이지만 `service.name`은 설정하지 않으며, sigiro는 서비스 이름이 없는 프로파일을 거부합니다. 아래의 `resource` 프로세서가 실행 파일 이름으로 서비스 이름을 채웁니다.

```yaml
receivers:
  profiling: {}

processors:
  resource:
    attributes:
      - key: service.name
        from_attribute: process.executable.name
        action: insert

exporters:
  otlp_http:
    endpoint: http://localhost:4318
    # For hosted sigiro:
    # endpoint: https://sigiro.com
    # headers:
    #   Authorization: "Bearer ${env:SIGIRO_ACCESS_TOKEN}"

service:
  pipelines:
    profiles:
      receivers: [profiling]
      processors: [resource]
      exporters: [otlp_http]
```

프로파일 기능 게이트를 켜고 Collector를 시작합니다.

```bash
sudo otelcol-ebpf-profiler --feature-gates=+service.profilesSupport --config config.yaml
```

`otlp_http`라는 이름 이전의 Collector 릴리스에서는 같은 익스포터를 `otlphttp`라고 부릅니다.

## OpenTelemetry Collector를 통해

이미 OTLP를 받고 있는 Collector가 프로파일을 sigiro로 전달할 수 있습니다. `otlp` 리시버에서 `otlp_http` 익스포터로 가는 `profiles` 파이프라인을 추가하고, `--feature-gates=+service.profilesSupport`로 Collector를 시작하십시오.

```yaml
receivers:
  otlp:
    protocols:
      http:
      grpc:

exporters:
  otlp_http/sigiro:
    endpoint: http://localhost:4318

service:
  pipelines:
    profiles:
      receivers: [otlp]
      exporters: [otlp_http/sigiro]
```

익스포터는 엔드포인트 뒤에 `/v1development/profiles`를 붙입니다. 전체 URL을 지정해야 한다면 대신 `profiles_endpoint`를 설정하십시오.

## HTTP 계약

- **경로:** `POST /v1development/profiles`.
- **본문:** protobuf `ExportProfilesServiceRequest`, 콘텐츠 유형은 `application/x-protobuf`입니다. 프로파일에는 JSON을 받지 않습니다.
- **서비스:** 모든 리소스에 `service.name` 속성이 있어야 합니다. 없으면 `400`을 반환하고 아무것도 저장하지 않습니다.
- **크기:** gzip을 받습니다. 요청 하나는 기본적으로 압축 해제 후 8 MiB로 제한되며, 이를 넘으면 `413`을 반환합니다.
- **오류:** 형식이 잘못된 프로파일이나 0이 아닌 샘플이 없는 프로파일은 `400`을 반환합니다. Better Auth OAuth/JWT 베어러 토큰이 없거나 잘못되면 `401`을, 용량 게이트는 `507`을 반환합니다. `2xx` 응답은 sigiro가 배치를 받아들였다는 뜻입니다.

## 동작 확인하기

프로파일러를 적어도 한 번의 업로드 간격 동안 실행하십시오. 그런 다음 메인 HTTP API에서 프로파일 행을 다시 읽습니다.

```bash
curl -s -X POST http://localhost:9999/v1/query \
  --data "SELECT service_name, profile_type, unit, count(*) AS samples
          FROM sigiro_profiles
          WHERE service_name = 'checkout'
            AND timestamp > now() - INTERVAL '1 hour'
          GROUP BY 1, 2, 3
          ORDER BY samples DESC"
```

호스티드 sigiro에서는 오리진을 바꾸고 `-H "Authorization: Bearer YOUR_TENANT_KEY"`를 추가하십시오. 결과가 비어 있지 않으면 내보내기, 프로파일 디코딩, HTTP 읽기가 확인된 것입니다. 대시보드의 **Profiles** 페이지에서 같은 데이터를 플레임 그래프로 볼 수 있습니다. 한 간격이 지나도 결과가 비어 있다면 대개 엔드포인트가 틀렸거나, 쿼리의 서비스 이름이 생산자와 맞지 않거나, 생산자 로그에 sigiro가 읽지 않는 형식에 대한 `400`이 있는 경우입니다.

## 다음 단계

- [호스티드 sigiro로 텔레메트리 전송](/docs/how-to/hosted-onboarding) — 호스팅 인증과 다른 텔레메트리 신호를 설정합니다
- [테이블 이해하기](/docs/explanation/tables) — 각 테이블에 어떤 텔레메트리가 저장되는지 확인합니다
