---
title: Jaeger와 함께 sigiro 시험 실행
description: 애플리케이션의 전송 대상을 바꾸지 않고 Jaeger v2 트레이스를 sigiro에도 보내는 방법.
sidebar:
  order: 8
---

이 절차는 마이그레이션이 아닌 **트레이스 전용** 평가입니다. 애플리케이션은 기존 Jaeger v2 OTLP 수신기(`4317`/`4318`)에 계속 전송합니다. 기존 Jaeger 저장소 exporter와 다른 파이프라인은 유지하고, 트레이스 파이프라인에 두 번째 exporter를 추가합니다. Jaeger는 로그나 메트릭을 받지 않습니다. 별도의 OpenTelemetry Collector가 Jaeger 앞에 있다면 해당 Collector의 트레이스 파이프라인에 적용하고 기존 Jaeger 출력은 그대로 유지하십시오.

## 1. 다른 호스트 포트로 sigiro 시작

Jaeger와 **같은 호스트**에서 `14317`, `14318`, `9999`가 비어 있는지 확인하십시오.

```bash
docker run -d --name sigiro-trial --restart unless-stopped \
  -p 127.0.0.1:14317:4317 \
  -p 127.0.0.1:14318:4318 \
  -p 127.0.0.1:9999:9999 \
  -v sigiro-trial-data:/var/lib/sigiro \
  ghcr.io/sigiroai/sigiro
curl --fail http://127.0.0.1:9999/health
```

`14317`은 gRPC용이고 이 예제는 HTTP `14318`만 사용합니다. `9999`가 점유되었으면 호스트 측을 `19999` 등으로 변경하고 아래 확인 요청에도 변경한 포트를 사용하십시오. 준비되지 않으면 `docker logs sigiro-trial`을 확인하십시오. 애플리케이션의 exporter 주소는 바꾸지 않습니다.

**보안 경계:** 이 `127.0.0.1`은 Collector에서 호스트의 루프백에 접근할 수 있는 경우에만 작동합니다. 별도 컨테이너/호스트는 이 주소로 접근할 수 없으므로 접근 가능한 사설 경로와 TLS/인증을 준비하십시오. 편의를 위해 Docker 바인딩을 `0.0.0.0`으로 바꾸지 마십시오. 셀프 호스팅 sigiro의 OTLP/쿼리 엔드포인트는 인증하지 않습니다. 신뢰할 수 없는 네트워크에 직접 노출하지 마십시오. 로컬 프로세스도 자동으로 신뢰하지 마십시오.

## 2. 기존 트레이스 설정에 미러 추가

먼저 원본 YAML을 백업하십시오. 다음은 **병합할 조각**이지 전체 설정이 아닙니다. 기존 수신기, 프로세서, exporter, Jaeger 저장소, 다른 파이프라인과 순서를 보존하십시오. 이 예제는 Jaeger 2.0.0의 컴포넌트 이름 `otlphttp`를 사용합니다. 최신 Collector는 `otlp_http`를 쓰며 예전 이름은 더 이상 권장되지 않는 별칭입니다. 실행 중인 바이너리로 검증하고 정의와 목록에 동일한 이름을 사용하십시오.

```yaml
exporters:
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
      # 실제 설정의 receiver 및 processor 목록을 유지하십시오.
      receivers: [otlp]
      processors: [batch]
      exporters: [jaeger_storage_exporter, otlphttp/sigiro]
```

`endpoint`는 기본 URL이고 exporter가 `/v1/traces`를 덧붙입니다. 경로를 중복해서 추가하지 마십시오. 평문 HTTP는 같은 호스트의 루프백에서만 사용하고 원격 경로에는 TLS/인증을 적용하십시오. 메모리 큐의 상한은 **1000개 요청**이며 1000개 스팬이 아닙니다. 재시도는 5분 후 끝납니다. 큐 포화, 재시도 만료, Collector 재시작 시 미러의 스팬이 유실될 수 있습니다. **무손실 전송을 보장하지 않습니다.** 큐 크기/용량과 송신·큐 삽입 실패 메트릭, Jaeger 수집 오류를 감시하십시오. 오류가 증가하면 롤백하십시오. Jaeger의 로그/메트릭 파이프라인에는 exporter를 추가하지 마십시오.

## 3. 기존 인스턴스를 재시작하기 전에 검증

편집된 파일을 **동일 버전** Jaeger v2/Collector 실행 파일의 `validate --config=<편집된-파일>`로 검사하십시오. 이 검사는 설정만 확인하며 연결이나 전송 성공은 검증하지 않습니다. 이어서 수정된 파일의 복사본으로 별도 시험 인스턴스를 실행하십시오. 복사본의 **모든** 리스너를 빈 포트로 변경해야 합니다: OTLP `4317`/`4318`, 쿼리 UI `16686`, Prometheus `8888`(사용한다면 `8889`), 헬스 `13133` 및 나머지 extension/receiver/query 포트. 시험 저장소를 운영 환경과 격리하고 운영 저장소에 쓰지 않도록 하십시오. 시험 추적이 두 시험 대상에 도착하는 것을 확인한 후 기존 인스턴스를 정상적인 변경 절차에 따라 재시작하십시오. Docker `-p` 변경만으로 공유 네트워크 이름 공간의 리스너나 저장소 충돌을 해결할 수 없습니다. 격리가 불가능하면 운영 재시작을 첫 구성 검사로 사용하지 마십시오.

## 4. 양쪽 확인

**기존** 앱/OTLP 주소를 통해 새로운 고유 스팬을 보내고 trace ID, 서비스, 시간을 기록합니다. Jaeger UI(보통 `16686`)에서 동일 ID의 스팬을 찾으십시오. sigiro에서 `curl --fail http://127.0.0.1:9999/v1/services`로 서비스를 확인한 뒤 ID를 질의하십시오. 테스트 시간을 포함하는 UTC 시작/끝 시각을 실제 값으로 바꾸십시오. `now()`는 DuckLake 파일 가지치기를 방해하고 시간 경계 질의 정책에 맞지 않습니다.

```bash
curl --fail -X POST http://127.0.0.1:9999/v1/query \
  --data "SELECT trace_id, span_name, timestamp FROM sigiro_spans
          WHERE trace_id = '<your-trace-id>'
            AND timestamp >= TIMESTAMP '<start-utc:YYYY-MM-DD HH:MM:SS>'
            AND timestamp < TIMESTAMP '<end-utc:YYYY-MM-DD HH:MM:SS>'"
```

한쪽이라도 스팬이 없으면 Jaeger/Collector 로그, 큐/실패 메트릭, sigiro 로그, 네트워크 이름 공간, 주소를 확인하십시오. 총계가 아닌 동일 trace ID를 비교하고 상위 수집 단계의 샘플링/필터링을 고려하십시오.

## 롤백

백업한 YAML로 되돌립니다(`traces.exporters` 목록과 `exporters` 맵 **양쪽**에서 `otlphttp/sigiro` 제거). 검증한 후 기존 절차로 재시작하고 새로운 트레이스가 Jaeger에 들어오는지 확인하십시오. 시험 컨테이너를 제거하십시오.

```bash
docker rm -f sigiro-trial
# 시험 데이터를 폐기하기로 결정했을 때만 실행 (복구 불가):
docker volume rm sigiro-trial-data
```

데이터를 보존하려면 마지막 명령을 생략하십시오. Jaeger를 중지하거나 애플리케이션 전송 대상을 바꿀 필요가 없습니다.

설정 원문: [Collector 설정 및 검증](https://opentelemetry.io/docs/collector/configuration/), [OTLP HTTP exporter](https://github.com/open-telemetry/opentelemetry-collector/blob/v0.112.0/exporter/otlphttpexporter/README.md), [큐 및 재시도](https://github.com/open-telemetry/opentelemetry-collector/blob/v0.112.0/exporter/exporterhelper/README.md), [문제 해결](https://opentelemetry.io/docs/collector/troubleshooting/), [Jaeger v2 API](https://www.jaegertracing.io/docs/2.0/architecture/apis/).
