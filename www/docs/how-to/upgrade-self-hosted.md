---
title: Upgrade a self-hosted server without assuming data compatibility
description: Back up the whole local data directory, choose a clean reset when compatibility is unknown, and keep a rollback copy.
sidebar:
  order: 8
---

## Compatibility contract (before 1.0)

**An in-place upgrade of an existing data directory is not guaranteed.** The local
DuckLake catalog and Parquet files are not a stable cross-release format contract.
Today the server has no Sigiro storage-format version or check that rejects an
incompatible catalog before listeners start. Startup uses `CREATE TABLE IF NOT
EXISTS`, which does not migrate existing tables. A ready health response does
**not** prove that every old table is compatible. Do not use an unqualified
“latest” image with important data.

A clean data directory is necessary when a release changes the format and cannot
read the previous one, when its release notes require a reset, or when you cannot
establish compatibility and cannot risk opening the old catalog. This is a
**planned loss of retained local history**, not an automatic migration. The
default seven-day retention is not a migration or a safety net. No supported
“drain and migrate” command or automatic conversion is provided. Downgrade on a
data directory opened by a newer release is **not supported**; use an untouched
copy made before that upgrade with the original release instead.

This page covers the **local self-hosted mode only** (no `SIGIRO_CATALOG_URL`).
A hosted/distributed PostgreSQL catalog, local tenant Parquet, or S3 data is not
covered by the local-directory archive below.

## Stop ingestion and back up before replacing the release

1. Record the exact running image digest or binary release and your settings,
   including data directory and retention. Stop senders or redirect OTLP traffic
   elsewhere. This is an operator pause, not a Sigiro drain API. Allow senders
   to finish; expect a gap or dropped telemetry if they keep sending while the
   server is offline.
2. Stop the server gracefully and wait for it to exit. Sigiro handles SIGTERM
   by stopping listeners and draining flusher tasks. **Do not copy a live volume**
   and do not force-kill the server. Check logs for a shutdown/drain failure; if
   it failed, do not assume the last writes are present in the copy.
3. Archive the **entire** data directory while it is stopped. It includes
   `admin.sqlite` (local administrative state), every project's
   `catalog.ducklake` **and** `catalog.ducklake.files/`, and the extension cache.
   A catalog file by itself cannot restore telemetry. Keep the archive and the
   original release away from the new deployment; restrict backup access.

For the named Docker volume from the [quickstart](/docs/tutorials/quickstart),
run these on the Docker host in a directory with enough free space:

```bash
docker stop --timeout 120 sigiro
test "$(docker inspect --format '{{.State.ExitCode}}' sigiro)" = 0 # inspect docker logs sigiro too
mkdir sigiro-backups # choose a new directory; do not overwrite an earlier archive
docker run --rm \
  --mount type=volume,source=sigiro-data,target=/source,readonly \
  --mount type=bind,source="$PWD/sigiro-backups",target=/backup \
  alpine sh -c 'cd /source && tar -cf /backup/sigiro-data.tar .'
tar -tf sigiro-backups/sigiro-data.tar > /dev/null
```

**Stop if the exit code is not zero or the archive check fails.** `tar -tf`
checks readability, not whether a restore works. Keep the source volume intact
until you have tested a recovery with the original release. For a native binary,
stop its service, check its exit status/logs, and archive the entire resolved
`SIGIRO_DATA_DIR` (or the platform data directory, normally
`~/.local/share/sigiro` on Unix) while it is stopped. Restart the original
release if you are not ready to make the change.

## Choose the new directory or an explicitly compatible release

When compatibility is unknown or a wipe is required, start the new release on
an **empty** directory or new Docker volume. The original volume stays offline
for rollback. For the Docker quickstart, after the backup:

```bash
docker rm sigiro # removes the stopped container, not sigiro-data
docker volume create sigiro-fresh
IMAGE='ghcr.io/sigiroai/sigiro@sha256:REPLACE_WITH_VERIFIED_DIGEST'
docker run -d --name sigiro --restart unless-stopped \
  -p 127.0.0.1:4317:4317 -p 127.0.0.1:4318:4318 -p 127.0.0.1:9999:9999 \
  -v sigiro-fresh:/var/lib/sigiro "$IMAGE"
curl --fail http://localhost:9999/health
```

Replace `REPLACE_WITH_VERIFIED_DIGEST` with the **specific** new image digest
chosen for this rollout. This is a clean reset: old telemetry and local admin
state do not appear in the new volume. Check server logs and send a test OTLP
record before resuming senders. If you need the old history instead, do **not**
point the new release at it unless that release explicitly documents and tests
compatibility with your exact prior release. Even then, retain the stopped
backup and verify queries and ingest before reopening traffic; this page cannot
promise that such a release exists.

## Roll back or restore

Stop the new server and redirect senders away first. To roll back, run the
**original pinned release** against the **untouched original** `sigiro-data`
volume with the original settings, and verify health plus real reads before
resuming traffic. The new volume's writes do not transfer back. If the original
volume is lost, restore the stopped full-directory archive into a **new empty**
volume/directory, then run the original release and original settings. Test
that restore in an isolated environment before depending on the backup; this
page does not claim a tested restore, an atomic live snapshot, or recovery of
writes made after the archive. Never merge individual Parquet files or an old
catalog into a newer directory, swap catalog backends, or delete the original
volume just because the new server returned `/health`.

The [quickstart](/docs/tutorials/quickstart) shows the Docker mount. The
format-version/fail-before-listeners work is tracked separately in issue #200;
this guide does not implement it.
