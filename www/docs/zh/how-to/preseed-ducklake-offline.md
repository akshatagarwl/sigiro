---
title: 为离线自托管服务预置 DuckLake
description: 在代理或离线环境中启动 Sigiro 前，准备与 DuckDB 版本和目标架构一致的扩展。
sidebar:
  order: 8
---

Sigiro 发布镜像**不包含 DuckLake**。初始化新的本地数据目录时，Sigiro 让 DuckDB 执行 `INSTALL ducklake; LOAD ducklake;`。下载和 HTTP 重定向由 DuckDB 处理；代理返回 HTTP 302 时，即使 `curl -L` 能下载，DuckDB 安装也可能失败。Sigiro 没有单独的重定向开关或离线下载器。请在联网机器上预置扩展缓存，再挂载到运行环境。

本流程仅适用于**本地文件目录**。PostgreSQL 目录还需要 DuckDB 的 `postgres` 扩展；S3 数据文件还需要 `httpfs`。使用分布式存储时，必须预置相同版本及平台的其他扩展。此单文件流程不能证明分布式部署已实现离线启动。

## 1. 选取准确的制品

当前 Sigiro 源码固定 DuckDB **`v2.0.0-alpha41489`**（不是 Rust `duckdb` crate 的版本）。DuckDB 缓存路径为 `<extension_directory>/<DuckDB version>/<platform>/`。发布镜像在 x86-64 上使用 `linux_amd64`，在 ARM64 上使用 `linux_arm64`。这是 DuckDB 平台名称，不是 Rust 目标三元组。Apple Silicon 原生构建使用 `osx_arm64`；不要把 macOS 扩展复制到 Linux 容器。每次升级 Sigiro 都应核对 `src/storage.rs` 中的固定版本；版本或目标平台变化后重新获取制品。

在**联网的 Linux 暂存机**上，设置目标平台及经独立审批的、对应此 URL 的压缩文件 SHA-256 摘要，然后运行：

```bash
set -eu
DUCKDB_VERSION=v2.0.0-alpha41489
PLATFORM=linux_amd64 # ARM64 发布镜像使用 linux_arm64
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

URL 遵循 [DuckDB 直接下载格式](https://duckdb.org/docs/current/extensions/advanced_installation_methods.html)；**必须解压** `.gz`。预期摘要应来自可信的制品审批流程；下载后计算未验证文件的哈希并把它当作预期值，不能构成校验。HTTPS 和 SHA-256 不能代替 DuckDB 的扩展签名校验。不要设置 `SIGIRO_ALLOW_UNSIGNED_EXTENSIONS`（默认 false），也不要自行生成 `.info` 元数据文件。若签名或版本校验失败，请获取匹配的官方制品，而不是关闭签名验证。

## 2. 挂载缓存并首次启动

把整个 `sigiro-extension-cache` 目录传输至离线主机。对 distroless 发布镜像，可将该**目录**挂载到 `/var/lib/sigiro/extensions`，并设置 `SIGIRO_EXTENSION_DIR=/var/lib/sigiro/extensions`。另需持久化可写的 `SIGIRO_DATA_DIR`（镜像默认 `/var/lib/sigiro`），以存放 DuckLake 目录、Parquet 文件和临时文件。容器以 **nonroot** 用户运行；两个挂载点都必须允许该用户进入并写入。即使扩展文件已存在，扩展根目录仍须可写：Sigiro 会创建目录，DuckDB 也可能写入缓存。只读挂载或覆盖并隐藏预置扩展目录的 `/var/lib/sigiro` 挂载不适用于本流程。不要依赖 distroless 镜像内存在 shell 或 `curl`。

原生可执行文件首次启动示例：

```bash
SIGIRO_DATA_DIR=/path/to/writable/data \
SIGIRO_EXTENSION_DIR=/path/to/writable/sigiro-extension-cache \
sigiro serve
```

本地模式下，未设置 `SIGIRO_EXTENSION_DIR` 时使用 `<SIGIRO_DATA_DIR>/extensions`。环境变量必须指向**缓存根目录**，而非 `v2.0.0-alpha41489/linux_amd64`。重启后保留缓存；重新构建容器也不会将 DuckLake 添加到镜像。

## 3. 验证存储，而不仅是监听端口

查看启动日志，确认包含 `loaded DuckDB extension: ducklake` 和 `default project initialized`。向端口 `4318` 发送 OTLP 样本后运行查询（等待写入刷新周期，再期待计数大于零）：

```bash
SINCE="$(date -u +'%Y-%m-%d %H:%M:%S')"
START_NS="$(date +%s)000000000"
END_NS="$(( $(date +%s) + 1 ))000000000"
curl -fsS -X POST http://127.0.0.1:4318/v1/traces \
  -H 'Content-Type: application/json' \
  --data "{\"resourceSpans\":[{\"resource\":{\"attributes\":[{\"key\":\"service.name\",\"value\":{\"stringValue\":\"offline-probe\"}}]},\"scopeSpans\":[{\"spans\":[{\"traceId\":\"33333333333333333333333333333333\",\"spanId\":\"4444444444444444\",\"name\":\"offline-probe\",\"kind\":1,\"startTimeUnixNano\":\"${START_NS}\",\"endTimeUnixNano\":\"${END_NS}\"}]}]}]}"
curl -fsS -X POST http://127.0.0.1:9999/v1/query \
  -H 'Content-Type: text/plain' \
  --data "SELECT count(*) AS n FROM lake.sigiro_spans WHERE timestamp >= TIMESTAMP '${SINCE}' AND service_name = 'offline-probe'"
```

在当前 main 中，`/health` 返回 `ready` **并不能证明存储或写入可用**。启动失败时在报告 ready 前退出的修复仍在 [#359](https://github.com/akshatagarwl/sigiro/pull/359) 审核中；不要假设当前二进制已包含此修复。检查日志并测试写入和查询。缺少扩展、版本或平台不匹配、签名无效或缓存不可写都是部署失败：修正制品或挂载后重启，而不是接受看似健康的监听端口。本文仅是运维绕过方案；扩展并未打包进镜像，也没有修复 DuckDB 的代理重定向。缓存布局参见 [DuckDB 扩展安装位置](https://duckdb.org/docs/current/extensions/installing_extensions.html)。
