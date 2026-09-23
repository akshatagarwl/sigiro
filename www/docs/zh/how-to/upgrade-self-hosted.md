---
title: 升级自托管服务器
description: 1.0 之前的数据兼容策略、完整备份、清空重建和回退。
sidebar:
  order: 8
---

## 1.0 之前的兼容策略

**不保证现有数据目录可以原地升级。** 本地 DuckLake 目录和 Parquet 文件
没有跨版本稳定格式的保证。目前 Sigiro 没有独立的存储格式版本，也没有在
监听端口启动前拒绝不兼容目录的检查。启动时的 `CREATE TABLE IF NOT
EXISTS` 不会迁移已有的表。`/health` 返回正常**不能**证明所有旧表都兼容。
不要让重要数据使用随时变化的 `latest` 镜像。

如果新版本无法读取旧格式、发行说明要求重置，或无法确认兼容性且不能
冒险打开旧目录，必须在**空目录**中重新启动。新环境不包含已保存的本地
历史数据。默认七天保留期不是迁移或备份方案。目前没有受支持的“排空后
迁移”命令或自动转换。**不支持**用旧版本打开已被新版本使用的数据目录。
回退时应使用升级前保留的未修改副本和原版本。

本指南仅针对没有设置 `SIGIRO_CATALOG_URL` 的**本地自托管模式**。
以下目录归档不能用于 PostgreSQL 目录、租户本地 Parquet 或 S3 数据。

## 更换版本前暂停发送并备份

1. 记录原镜像的精确摘要（或二进制版本）、配置、数据目录和保留期。
   停止 OTLP 发送端或将流量转向其他目的地，并等待正在发送的请求结束。
   这是运维侧的暂停，不是 Sigiro 提供的迁移排空 API。如果离线时仍有
   数据发送，可能会出现缺口或丢失。
2. 用 SIGTERM 正常停止服务器并等待退出。Sigiro 会停止监听并排空
   写入任务。**不要复制运行中的卷，也不要强制终止。** 检查退出码和
   日志中的关闭或排空失败；失败时不能假定最后的写入已进入备份。
3. 停止期间归档**整个**数据目录，包括本地管理状态 `admin.sqlite`、
   每个项目的 `catalog.ducklake` **及** `catalog.ducklake.files/`，以及
   扩展缓存。只有目录文件不能恢复遥测数据。妥善保管归档和原始版本。

对于[快速入门](/docs/zh/tutorials/quickstart)中的 Docker 卷
`sigiro-data`，请在 Docker 主机的空闲磁盘足够时运行：

```bash
docker stop --timeout 120 sigiro
test "$(docker inspect --format '{{.State.ExitCode}}' sigiro)" = 0 # 同时检查 docker logs sigiro
mkdir sigiro-backups # 换用新目录名，避免覆盖先前的归档
docker run --rm \
  --mount type=volume,source=sigiro-data,target=/source,readonly \
  --mount type=bind,source="$PWD/sigiro-backups",target=/backup \
  alpine sh -c 'cd /source && tar -cf /backup/sigiro-data.tar .'
tar -tf sigiro-backups/sigiro-data.tar > /dev/null
```

退出码非零或归档检查失败时立即停止操作。`tar -tf` 只能检查可读性，
不能证明恢复成功。在隔离环境中用原版本测试恢复之前，请保留原卷，不要
只依赖归档。如果使用二进制部署，停止服务并确认退出状态，然后归档完整
的 `SIGIRO_DATA_DIR`（未设置时 Unix 系统通常为
`~/.local/share/sigiro`）。尚未准备好升级时，重启原版本。

## 在新空卷上启动

兼容性未知或要求清空时，保留原卷以便回退，用**新空卷**运行新版本。
在备份完成后，Docker 用户可运行：

```bash
docker rm sigiro # 只删除已停止的容器，不删除 sigiro-data
docker volume create sigiro-fresh
IMAGE='ghcr.io/sigiroai/sigiro@sha256:REPLACE_WITH_VERIFIED_DIGEST'
docker run -d --name sigiro --restart unless-stopped \
  -p 127.0.0.1:4317:4317 -p 127.0.0.1:4318:4318 -p 127.0.0.1:9999:9999 \
  -v sigiro-fresh:/var/lib/sigiro "$IMAGE"
curl --fail http://localhost:9999/health
```

将 `REPLACE_WITH_VERIFIED_DIGEST` 换成此次升级选定的**固定镜像摘要**。
新卷没有旧遥测和本地管理状态。检查日志并发送一条测试 OTLP 数据，
确认接收后再恢复发送端。如果要保留旧数据，只有当新版本**明确说明并
测试**了与你使用的旧版本兼容，才能让它打开旧目录。即使有此保证，
也要保存停止后的备份，并在开放流量前验证查询和接收。

## 回退或恢复

先停止新服务器并转移发送端。使用**原来的固定版本**、**未修改的**
`sigiro-data` 卷和原配置启动，检查健康状态并读取真实数据后再恢复
流量。写入新卷的数据不会回到旧卷。若原卷丢失，请把停止时创建的
完整归档恢复到**新的空**卷或目录，然后用原版本和原配置启动。
依赖备份前请先在隔离环境测试恢复。本指南不宣称恢复已测试、运行中
快照具有原子性，也不保证归档之后写入的数据可以恢复。不要将单个
Parquet 文件或旧目录与新目录混合，不要因为 `/health` 正常就删除原卷。

存储格式版本和启动监听前拒绝不兼容目录属于独立的 issue #200；
本指南没有实现该功能。
