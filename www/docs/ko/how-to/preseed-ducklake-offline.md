---
title: 오프라인 자체 호스팅 서버용 DuckLake 미리 배치하기
description: 프록시 또는 오프라인 환경에서 시작하기 전에 DuckDB 버전과 아키텍처에 맞는 확장을 배치합니다.
sidebar:
  order: 8
---

Sigiro 릴리스 이미지에는 **DuckLake가 포함되지 않습니다**. 새 로컬 데이터 디렉터리를 초기화할 때 Sigiro는 DuckDB에 `INSTALL ducklake; LOAD ducklake;`를 실행하도록 합니다. 다운로드와 HTTP 리디렉션 처리는 DuckDB가 담당합니다. 프록시가 HTTP 302를 반환하면 `curl -L`로 다운로드가 가능해도 DuckDB 설치는 실패할 수 있습니다. Sigiro에 별도 리디렉션 설정이나 오프라인 다운로더는 없습니다. 인터넷에 연결된 호스트에서 캐시를 준비해 런타임에 마운트하십시오.

이 절차는 **로컬 파일 카탈로그**에만 해당합니다. PostgreSQL 카탈로그에는 DuckDB의 `postgres` 확장이, S3 데이터 파일에는 `httpfs` 확장이 추가로 필요합니다. 분산 스토리지를 사용한다면 같은 버전/플랫폼의 확장도 배치하십시오. 이 단일 파일 절차만으로 분산 배포의 오프라인 동작을 보장하지 않습니다.

## 1. 정확한 아티팩트 선택

현재 Sigiro 소스에 고정된 DuckDB 버전은 **`v2.0.0-alpha41489`**입니다(Rust `duckdb` 크레이트 버전이 아님). DuckDB 캐시 경로는 `<extension_directory>/<DuckDB version>/<platform>/`입니다. 릴리스 이미지의 x86-64에는 `linux_amd64`, ARM64에는 `linux_arm64`를 선택하십시오. 이는 Rust 타깃 트리플이나 Docker 아키텍처 값이 아닌 DuckDB 플랫폼 이름입니다. Apple Silicon 네이티브 빌드는 `osx_arm64`가 필요합니다. macOS 확장을 Linux 컨테이너에 복사하지 마십시오. Sigiro를 업그레이드할 때마다 `src/storage.rs`의 고정 버전을 확인하고, 버전이나 대상 플랫폼이 바뀌면 새 아티팩트를 준비하십시오.

**인터넷에 연결된 Linux 준비 호스트**에서 플랫폼과 해당 URL의 압축 파일에 대해 독립적으로 승인된 SHA-256 해시를 지정한 뒤 실행하십시오.

```bash
set -eu
DUCKDB_VERSION=v2.0.0-alpha41489
PLATFORM=linux_amd64 # ARM64 릴리스 이미지에서는 linux_arm64
: "${DUCKLAKE_GZ_SHA256:?Set the approved SHA-256 digest for this exact URL}"
STAGE="${PWD}/sigiro-extension-cache"
DEST="${STAGE}/${DUCKDB_VERSION}/${PLATFORM}"
mkdir -p "$DEST"
curl --fail --location --proto '=https' --proto-redir '=https' \
  "https://extensions.duckdb.org/${DUCKDB_VERSION}/${PLATFORM}/ducklake.duckdb_extension.gz" \
  --output "$DEST/ducklake.duckdb_extension.gz"
printf '%s  %s\n' "$DUCKLAKE_GZ_SHA256" "$DEST/ducklake.duckdb_extension.gz" | sha256sum -c -
gzip -dc "$DEST/ducklake.duckdb_extension.gz" > "$DEST/ducklake.duckdb_extension"
rm "$DEST/ducklake.duckdb_extension.gz"
test -s "$DEST/ducklake.duckdb_extension"
```

URL은 [DuckDB 직접 다운로드 형식](https://duckdb.org/docs/current/extensions/advanced_installation_methods.html)을 따릅니다. `.gz` 파일은 **압축을 풀어야** 합니다. 예상 해시는 신뢰할 수 있는 아티팩트 승인 절차에서 얻으십시오. 검증되지 않은 다운로드의 해시를 계산해 그 값을 예상 해시로 사용하면 검증이 아닙니다. HTTPS와 SHA-256은 DuckDB 확장 서명 검증을 대체하지 않습니다. `SIGIRO_ALLOW_UNSIGNED_EXTENSIONS`는 설정하지 마십시오(기본값 false). `.info` 메타데이터 파일을 직접 만들지 마십시오. 서명이나 버전 검증에 실패하면 서명 검사를 끄지 말고 일치하는 공식 아티팩트를 구하십시오.

## 2. 캐시 마운트 및 첫 시작

`sigiro-extension-cache` 디렉터리를 그대로 오프라인 호스트로 전송합니다. distroless 릴리스 이미지에서는 예를 들어 해당 **디렉터리**를 `/var/lib/sigiro/extensions`에 마운트하고 `SIGIRO_EXTENSION_DIR=/var/lib/sigiro/extensions`를 설정합니다. DuckLake 카탈로그, Parquet, 임시 파일을 위해 별도의 쓰기 가능한 `SIGIRO_DATA_DIR`(이미지의 기본값 `/var/lib/sigiro`)을 영구 보관하십시오. 컨테이너는 **nonroot** 사용자로 실행되므로 그 사용자가 두 마운트를 탐색하고 쓸 수 있어야 합니다. 미리 배치한 파일이 있더라도 확장 루트에는 쓰기 권한이 필요합니다. Sigiro가 디렉터리를 생성하고 DuckDB가 캐시에 기록할 수 있습니다. 읽기 전용 마운트나 기존 확장 디렉터리를 가리는 `/var/lib/sigiro` 마운트는 이 절차와 맞지 않습니다. distroless 이미지 내부의 셸이나 `curl`에 의존하지 마십시오.

네이티브 바이너리의 첫 실행 예:

```bash
SIGIRO_DATA_DIR=/path/to/writable/data \
SIGIRO_EXTENSION_DIR=/path/to/writable/sigiro-extension-cache \
sigiro serve
```

로컬 모드에서 `SIGIRO_EXTENSION_DIR`를 설정하지 않으면 `<SIGIRO_DATA_DIR>/extensions`가 사용됩니다. 환경 변수에는 `v2.0.0-alpha41489/linux_amd64`가 아니라 그 **상위 캐시 루트**를 지정하십시오. 재시작 후에도 캐시를 보존하십시오. 컨테이너를 다시 빌드한다고 DuckLake가 이미지에 포함되지는 않습니다.

## 3. 리스너뿐 아니라 스토리지 확인

시작 로그에서 `loaded DuckDB extension: ducklake`와 `default project initialized`를 확인하십시오. 포트 `4318`에 OTLP 샘플을 보낸 뒤 쿼리하십시오(0보다 큰 행 수를 기대하기 전 수집 플러시 간격을 기다리십시오):

```bash
curl -fsS -X POST http://127.0.0.1:4318/v1/traces \
  -H 'Content-Type: application/json' \
  --data '{"resourceSpans":[{"resource":{"attributes":[{"key":"service.name","value":{"stringValue":"offline-probe"}}]},"scopeSpans":[{"spans":[{"traceId":"33333333333333333333333333333333","spanId":"4444444444444444","name":"offline-probe","kind":1,"startTimeUnixNano":"1788415000000000000","endTimeUnixNano":"1788415000100000000"}]}]}]}'
curl -fsS -X POST http://127.0.0.1:9999/v1/query \
  -H 'Content-Type: text/plain' \
  --data 'SELECT count(*) AS n FROM lake.sigiro_spans'
```

현재 main에서는 `/health`의 `ready` **응답만으로 스토리지 및 수집 동작을 증명할 수 없습니다**. 시작 실패 시 ready 이전에 종료하는 수정은 [#359](https://github.com/akshatagarwl/sigiro/pull/359)에서 검토 중입니다. 사용 중인 바이너리에 포함되었다고 가정하지 마십시오. 로그와 두 경로를 직접 확인하십시오. 확장 누락, 버전/플랫폼 불일치, 서명 오류 또는 쓰기 불가능한 캐시는 배포 실패입니다. 아티팩트나 마운트를 수정하고 재시작하십시오. 이 문서는 운영자 우회 절차이지 이미지에 확장을 포함하거나 DuckDB 프록시 리디렉션을 수정하는 기능이 아닙니다. 경로 규칙은 [DuckDB 확장 설치 위치](https://duckdb.org/docs/current/extensions/installing_extensions.html)를 참조하십시오.
