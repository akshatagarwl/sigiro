---
title: Jaeger와 함께 sigiro 시험 실행
description: 애플리케이션의 대상을 바꾸지 않고 기존 Jaeger v2 Collector에서 루프백 전용 sigiro 인스턴스로 OTLP 트레이스를 미러링합니다.
sidebar:
  order: 8
---

이것은 마이그레이션이 아니라 **트레이스만의 평가**입니다. 애플리케이션은 계속 `4317`/`4318`의 기존 Jaeger v2 OTLP 수신기로 보내게 두십시오. 그 수신기의 **traces** 파이프라인에 sigiro를 두 번째 익스포터로 추가하고, Jaeger의 스토리지 익스포터와 다른 모든 파이프라인은 유지하십시오. Jaeger는 트레이스 데이터를 받으며 OTLP 로그나 메트릭은 받지 않습니다. Jaeger 앞에 별도의 OpenTelemetry Collector가 있다면 기존 Jaeger 목적지를 유지한 채 같은 traces 파이프라인 변경을 그곳에서 하십시오.

## 1. 다른 호스트 포트로 sigiro 시작하기

Jaeger와 **같은 호스트**에서 `14317`, `14318`, `9999`가 비어 있는 상태로 실행합니다.

```bash
docker run -d --name sigiro-trial --restart unless-stopped \
  -p 127.0.0.1:14317:4317 \
  -p 127.0.0.1:14318:4318 \
  -p 127.0.0.1:9999:9999 \
  -v sigiro-trial-data:/var/lib/sigiro \
  ghcr.io/sigiroai/sigiro
curl --fail http://127.0.0.1:9999/health
```

`14317`은 다시 매핑한 gRPC 포트이며, 이 가이드는 `14318`의 OTLP HTTP만 사용합니다. `9999`가 사용 중이면 그 **호스트** 쪽도 다시 매핑하고(예: `-p 127.0.0.1:19999:9999`) 아래 확인에서 그 포트를 사용하십시오. 상태 확인이 성공할 때까지 기다리거나 `docker logs sigiro-trial`을 살펴보십시오. 애플리케이션 익스포터의 엔드포인트는 바꾸지 마십시오.

**네트워크 경계:** 이 예시는 Jaeger/Collector가 자신의 네트워크 네임스페이스에서 호스트의 `127.0.0.1:14318`에 도달할 수 있다고 가정합니다. 다른 컨테이너나 다른 호스트의 Collector는 그 루프백 주소를 쓸 수 없습니다. TLS/인증 프록시(또는 동등한 네트워크 통제)를 거치는 사설 경로를 마련하고 도달 가능한 그 주소를 사용하십시오. sigiro 자체는 인증을 제공하지 않습니다. 이것을 동작시키려고 Docker 바인딩을 `0.0.0.0`으로 바꾸지 마십시오.
셀프 호스팅 sigiro는 OTLP나 쿼리 요청을 인증하지 않으므로 이 포트들을 신뢰할 수 없는 네트워크에 직접 공개하지 마십시오. 루프백 바인딩만으로 로컬 프로세스가 신뢰할 수 있게 되지는 않습니다.

## 2. 기존 traces 파이프라인에 상한이 있는 미러 추가하기

Jaeger v2/Collector YAML을 백업하십시오. 다음 항목을 그 안에 **병합**하십시오. 이것은 조각이며 대체 설정이 아닙니다. 기존 수신기, 프로세서, 익스포터, 확장, Jaeger 스토리지, 트레이스가 아닌 파이프라인은 그대로 두십시오. traces 파이프라인에 익스포터나 프로세서가 더 있다면 기존 순서대로 유지하십시오. 이 예시는 Jaeger **2.0.0** 컴포넌트 ID를 사용합니다. 그 릴리스는 OTLP HTTP 익스포터를 `otlphttp`라고 부릅니다. 더 새로운 Collector 배포판은 `otlp_http`라고 부릅니다(예전 철자는 사용 중단된 별칭입니다). 설치된 바이너리로 `validate`를 실행해 확인하고, 받아들여지는 이름을 두 곳 모두에 일관되게 사용하십시오.

```yaml
exporters:
  # Add alongside the existing jaeger_storage_exporter; do not replace it.
  otlphttp/sigiro:
    endpoint: http://127.0.0.1:14318
    sending_queue:
      enabled: true
      queue_size: 1000
    retry_on_failure:
      enabled: true
      initial_interval: 5s
      max_interval: 30s
      max_elapsed_time: 300s

service:
  pipelines:
    traces:
      # Preserve the actual receiver and processor lists from your config.
      receivers: [otlp]
      processors: [batch]
      exporters: [jaeger_storage_exporter, otlphttp/sigiro]
```

`endpoint`는 OTLP/HTTP **기본** URL이며 익스포터가 `/v1/traces`를 덧붙입니다. 여기에 `/v1/traces`를 추가하거나, 전체 경로 없이 신호별 `traces_endpoint`를 사용하지 마십시오. 여기서 평문 HTTP는 같은 호스트의 루프백에서만 적절합니다. `http://` 엔드포인트에는 `tls: { insecure: true }`가 필요하지 않습니다. 원격 사설 목적지라면 대신 양쪽에 TLS와 인증을 설정하십시오.

큐는 **메모리에** 있으며 최대 1,000개의 스팬이 아니라 최대 1,000개의 요청을 담습니다. 재시도는 배치마다 5분 뒤 끝납니다. 큐가 가득 차거나, 재시도가 만료되거나, Collector가 재시작하면 미러링된 스팬이 사라질 수 있습니다. 손실이 없음을 보장하는 팬아웃이 **아닙니다**. Collector의 익스포터 큐 용량/크기와 전송 실패 및 인큐 실패 메트릭을 모니터링하고, 부하와 메모리에 맞게 큐 크기를 정하십시오. 그런 실패나 Jaeger 수집 오류가 늘어나면 시험을 멈추고 원래 설정을 복원하십시오. Jaeger의 메트릭이나 로그 파이프라인에는 sigiro 익스포터를 추가하지 마십시오.

## 3. 운영 중인 인스턴스를 바꾸기 전에 검증하기

먼저 **같은 Jaeger v2 또는 Collector 바이너리/이미지 버전**으로 편집한 파일에 `validate --config=<edited-file>` 명령을 실행하십시오. 검증은 설정을 확인할 뿐 네트워크 접근, 전달, 스토리지는 확인하지 **않습니다**. 그런 다음 편집한 설정의 복사본으로 그 버전의 별도 스크래치 인스턴스를 시작하십시오. 스크래치 복사본에서는 OTLP `4317`/`4318`, Jaeger 쿼리/UI `16686`, 내부 Prometheus 메트릭 `8888`(그리고 있다면 `8889`), 상태 `13133`, 그 밖의 활성 확장, 수신기, 쿼리 리스너를 포함해 **모든** 리스너를 사용하지 않는 포트로 다시 매핑하십시오. 스토리지는 운영 환경과 분리하거나 읽기 전용 테스트 백엔드를 사용하십시오. 스크래치 익스포터가 운영 스토리지에 쓰지 않게 하십시오. 운영 인스턴스를 재시작하기 전에 스크래치 프로세스가 시작되고 테스트 트레이스를 두 스크래치 목적지 모두로 보내는지 확인하십시오. Docker `-p` 재매핑만으로는 공유 네트워크 네임스페이스 안의 리스너가 바뀌지 않으며 스토리지 충돌도 막지 **못합니다**. 스크래치 격리를 마련할 수 없다면 운영 재시작을 첫 구문 테스트로 삼지 마십시오.

스크래치 확인이 통과하면 평소의 통제된 Jaeger/Collector 재시작이나 롤아웃으로 편집한 설정을 적용하십시오. 즉시 시작 로그와 기존 Jaeger 쿼리 UI를 확인하십시오. 롤백을 위해 원래 YAML을 보관하십시오.

## 4. 두 백엔드 모두 확인하기

**기존** 앱이나 그 원래 OTLP 엔드포인트를 통해(sigiro로 직접이 아니라) 고유한 이름의 **새** 테스트 스팬을 생성하십시오. 트레이스 ID, 서비스 이름, 시각을 기록하십시오. 배치/내보내기 지연 후:

1. 기존 Jaeger UI(보통 포트 `16686`)에서 그 트레이스 ID를 찾아 스팬이 있는지 확인하십시오. 이것으로 기존 스토리지 경로가 여전히 동작함이 입증됩니다. Collector가 성공적으로 시작했다는 것만으로는 입증되지 않습니다.
2. sigiro에서는 `curl --fail http://127.0.0.1:9999/v1/services`로 서비스를 확인하십시오. `POST /v1/query`로 테스트 구간 안에서 정확한 트레이스 ID를 쿼리하거나, 해당 스팬을 찾으십시오. 예를 들어 트레이스 ID와 테스트 스팬을 감싸는 UTC 구간을 바꿔 넣은 뒤 실행합니다. 리터럴 범위를 사용하십시오. `now()`는 DuckLake 파일 가지치기를 막을 수 있으며, 범위 쿼리 정책은 반열린 리터럴 구간을 요구합니다.

   ```bash
   curl --fail -X POST http://127.0.0.1:9999/v1/query \
     --data "SELECT trace_id, span_name, timestamp FROM sigiro_spans
             WHERE trace_id = '<your-trace-id>'
               AND timestamp >= TIMESTAMP '<start-utc:YYYY-MM-DD HH:MM:SS>'
               AND timestamp < TIMESTAMP '<end-utc:YYYY-MM-DD HH:MM:SS>'"
   ```

어느 한 백엔드에 스팬이 없다면 결론을 내리기 전에 Jaeger/Collector 로그, 익스포터 실패/큐 메트릭, sigiro 로그, 네트워크 네임스페이스, 엔드포인트를 살펴보십시오. 전체 개수만이 아니라 실제 트레이스 ID를 비교하십시오. 상위의 샘플링이나 필터링이 두 목적지 모두에 영향을 줄 수 있습니다.

## 롤백

백업한 YAML을 복원하고(`otlphttp/sigiro`를 **traces 익스포터 목록과 익스포터 맵 양쪽에서** 제거), 검증한 뒤, 같은 통제된 재시작/롤아웃을 사용하십시오. Jaeger의 원래 수신기, 스토리지 익스포터, 애플리케이션 설정, 다른 파이프라인은 그대로 남습니다. Jaeger에서 새 트레이스를 확인하십시오. 그런 다음 시험 인스턴스를 멈추고 제거하십시오.

```bash
docker rm -f sigiro-trial
# Only after deciding to discard the trial's stored telemetry:
docker volume rm sigiro-trial-data
```

볼륨 제거는 되돌릴 수 없습니다. 시험 데이터를 유지하려면 마지막 명령을 생략하십시오. 롤백 과정에서 Jaeger를 멈추거나 애플리케이션의 대상을 바꾸지 마십시오.

## 설정 참조

- [Collector configuration and validation](https://opentelemetry.io/docs/collector/configuration/)
- [Jaeger 2.0.0 exporter factory](https://github.com/jaegertracing/jaeger/blob/v2.0.0/cmd/jaeger/internal/components.go) 및 [Collector 0.112.0 component ID](https://github.com/open-telemetry/opentelemetry-collector/blob/v0.112.0/exporter/otlphttpexporter/internal/metadata/generated_status.go)
- [OTLP HTTP exporter settings (Collector 0.112.0)](https://github.com/open-telemetry/opentelemetry-collector/blob/v0.112.0/exporter/otlphttpexporter/README.md)
- [Exporter queue/retry behavior (Collector 0.112.0)](https://github.com/open-telemetry/opentelemetry-collector/blob/v0.112.0/exporter/exporterhelper/README.md)
- [Collector troubleshooting and exporter metrics](https://opentelemetry.io/docs/collector/troubleshooting/)
- [Jaeger v2 API/port reference](https://www.jaegertracing.io/docs/2.0/architecture/apis/)
