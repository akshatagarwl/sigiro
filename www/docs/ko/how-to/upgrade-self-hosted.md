---
title: 셀프 호스팅 서버 업그레이드
description: 1.0 이전 데이터 호환성, 전체 백업, 초기화 및 이전 버전 복구.
sidebar:
  order: 8
---

## 1.0 이전 호환성 정책

**기존 데이터 디렉터리의 인플레이스 업그레이드는 보장되지 않습니다.** 로컬
DuckLake 카탈로그와 Parquet 파일의 릴리스 간 저장 형식 호환성은 보장하지
않습니다. 현재 Sigiro 전용 저장 형식 버전도, 호환되지 않는 카탈로그를
리스너 시작 전에 거부하는 검사도 없습니다. 시작 시 실행하는 `CREATE TABLE
IF NOT EXISTS`는 기존 테이블을 마이그레이션하지 않습니다. `/health`가
정상이어도 모든 기존 테이블의 호환성을 증명하지 않습니다. 중요한 데이터에
변하는 `latest` 이미지를 사용하지 마세요.

새 릴리스가 이전 형식을 읽을 수 없거나, 릴리스 노트가 초기화를 요구하거나,
호환성을 확인할 수 없어 기존 카탈로그를 열 위험을 감수할 수 없다면
**빈 디렉터리에서 다시 시작**해야 합니다. 새 환경에서 기존 로컬 기록을
잃습니다. 기본 보존 기간 7일은 마이그레이션이나 백업 방법이 아닙니다.
지원되는 '드레인 후 마이그레이션' 명령이나 자동 변환은 없습니다. 새
릴리스가 연 데이터 디렉터리의 **다운그레이드는 지원되지 않습니다.** 이전
릴리스와 업그레이드 전에 보관한 변경되지 않은 사본으로 복구하세요.

이 절차는 `SIGIRO_CATALOG_URL`이 없는 **로컬 셀프 호스팅 모드**에만
적용됩니다. PostgreSQL 카탈로그, 테넌트 로컬 Parquet 또는 S3 데이터에는
아래 로컬 디렉터리 백업을 적용하지 마세요.

## 릴리스 교체 전에 전송 중단 및 백업

1. 실행 중인 정확한 이미지 다이제스트(또는 바이너리 릴리스), 설정,
   데이터 디렉터리와 보존 기간을 기록합니다. OTLP 전송을 중단하거나 다른
   대상으로 돌리고 진행 중인 전송을 마칠 때까지 기다립니다. 이는 운영자의
   중단 조치이며 Sigiro의 마이그레이션 드레인 API가 아닙니다. 오프라인 동안
   계속 전송하면 데이터가 누락될 수 있습니다.
2. SIGTERM으로 서버를 정상 종료하고 완료를 기다립니다. Sigiro는
   리스너를 중단하고 쓰기 작업을 드레인합니다. **실행 중인 볼륨을 복사하거나
   강제 종료하지 마세요.** 종료 코드와 로그에서 종료/드레인 실패를 확인하고,
   실패했다면 마지막 쓰기가 백업에 있다고 가정하지 마세요.
3. 서버가 멈춘 상태에서 **전체** 데이터 디렉터리를 보관합니다. 로컬 관리
   상태 `admin.sqlite`, 모든 프로젝트의 `catalog.ducklake` 및
   `catalog.ducklake.files/`, 확장 캐시를 포함합니다. 카탈로그 파일만으로는
   원격 측정 데이터를 복원할 수 없습니다. 백업과 원래 릴리스를 안전하게
   별도로 보관하세요.

[빠른 시작](/docs/ko/tutorials/quickstart)의 Docker 볼륨 `sigiro-data`를
사용한다면 Docker 호스트에서 충분한 여유 공간을 확보한 다음 실행하세요.

```bash
docker stop --timeout 120 sigiro
test "$(docker inspect --format '{{.State.ExitCode}}' sigiro)" = 0 # docker logs sigiro도 확인
mkdir sigiro-backups # 기존 아카이브를 덮어쓰지 않도록 새 이름 선택
docker run --rm \
  --mount type=volume,source=sigiro-data,target=/source,readonly \
  --mount type=bind,source="$PWD/sigiro-backups",target=/backup \
  alpine sh -c 'cd /source && tar -cf /backup/sigiro-data.tar .'
tar -tf sigiro-backups/sigiro-data.tar > /dev/null
```

종료 코드가 0이 아니거나 아카이브 검사가 실패하면 중단하세요. `tar -tf`는
복원 성공을 증명하지 않습니다. 이전 볼륨을 보존하고 원래 릴리스로 격리
환경에서 복원을 시험한 뒤 백업에 의존하세요. 바이너리는 서비스를 종료하고
종료 상태를 확인한 뒤, 중단된 상태에서 `SIGIRO_DATA_DIR` 전체(미설정 시
Unix에서는 보통 `~/.local/share/sigiro`)를 보관하세요. 준비되지 않았다면
기존 릴리스를 다시 시작하세요.

## 새 빈 볼륨에서 시작

호환성이 불명확하거나 초기화가 필요한 경우 이전 볼륨은 롤백용으로 남겨
두고 **새 빈** 볼륨을 사용합니다. 백업 후 Docker에서는:

```bash
docker rm sigiro # 중지된 컨테이너만 제거하며 sigiro-data는 유지
docker volume create sigiro-fresh
IMAGE='ghcr.io/sigiroai/sigiro@sha256:REPLACE_WITH_VERIFIED_DIGEST'
docker run -d --name sigiro --restart unless-stopped \
  -p 127.0.0.1:4317:4317 -p 127.0.0.1:4318:4318 -p 127.0.0.1:9999:9999 \
  -v sigiro-fresh:/var/lib/sigiro "$IMAGE"
curl --fail http://localhost:9999/health
```

`REPLACE_WITH_VERIFIED_DIGEST`를 선택한 **고정된 새 이미지 다이제스트**로
바꾸세요. 기존 원격 측정 및 관리 상태는 새 볼륨으로 이전되지 않습니다.
로그와 테스트 OTLP 레코드 수신을 확인한 후 전송을 재개하세요. 이전 기록이
필요해도 새 릴리스에서 해당 이전 릴리스와의 호환성을 **명시하고 시험**하지
않았다면 이전 볼륨을 열지 마세요. 호환성이 확인돼도 중지 상태의 백업을
보존하고, 트래픽 재개 전에 쿼리와 수집을 확인하세요.

## 롤백 및 복원

먼저 새 서버를 중단하고 전송을 다른 곳으로 돌리세요. **원래 고정 릴리스**와
**변경되지 않은** `sigiro-data` 볼륨 및 원래 설정으로 실행하고, 상태와
실제 데이터 읽기를 확인한 뒤 전송을 재개하세요. 새 볼륨의 쓰기는 이전
볼륨으로 돌아가지 않습니다. 원래 볼륨을 잃었다면 중단 중 만든 전체
아카이브를 **새 빈** 볼륨/디렉터리에 복원한 다음 원래 릴리스와 설정으로
시작하세요. 백업에 의존하기 전에 격리 환경에서 복원을 시험하세요. 이
문서는 복원 시험 완료, 실행 중 원자적 스냅샷 또는 백업 이후의 쓰기 복구를
보장하지 않습니다. 개별 Parquet 파일이나 기존 카탈로그를 새 디렉터리에
섞지 마세요. `/health`가 정상이라는 이유만으로 이전 볼륨을 지우지 마세요.

저장 형식 버전과 리스너 시작 전 거부는 별도의 issue #200 과제입니다.
이 가이드는 그 기능을 구현하지 않습니다.
