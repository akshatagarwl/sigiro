---
title: Pre-seed DuckLake for an offline self-hosted server
description: Stage the signed DuckDB extension for the pinned version and target architecture before starting sigiro behind a proxy or without repository access.
sidebar:
  order: 8
---

Sigiro's release image **does not bundle DuckLake**. On a new local data directory,
Sigiro asks DuckDB to run `INSTALL ducklake; LOAD ducklake;` during project
initialization. DuckDB owns that download and its HTTP redirect handling; a proxy
that returns HTTP 302 may prevent installation even if `curl -L` succeeds. Sigiro
has no separate redirect setting or offline downloader. Pre-seed the extension
cache on a connected machine, then mount it into the runtime.

This procedure covers the **local file catalog**. A PostgreSQL catalog additionally
needs DuckDB's `postgres` extension; S3 data files additionally need `httpfs`.
Stage those for the same version/platform if using distributed storage. Do not
use this single-file recipe as proof of an offline distributed deployment.

## 1. Select the exact artifact

The current Sigiro source pins DuckDB **`v2.0.0-alpha41489`** (not the
`duckdb` Rust crate version). DuckDB caches extensions below
`<extension_directory>/<DuckDB version>/<platform>/`. For the release image,
choose `linux_amd64` on x86-64 or `linux_arm64` on ARM64. These are DuckDB
platform names, not Rust target triples or Docker's `amd64`/`arm64` alone.
A native Apple Silicon build instead needs `osx_arm64`; do not copy a macOS
extension into a Linux container. Verify the pin in `src/storage.rs` for every
Sigiro upgrade and fetch new artifacts when the pin or target platform changes.

On a **connected Linux staging host**, set the platform and an independently
approved SHA-256 digest for the compressed artifact before running:

```bash
set -eu
DUCKDB_VERSION=v2.0.0-alpha41489
PLATFORM=linux_amd64 # use linux_arm64 for the ARM64 release image
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

The URL follows [DuckDB's direct-download format](https://duckdb.org/docs/current/extensions/advanced_installation_methods.html);
`.gz` **must be decompressed**. Obtain the digest through your trusted artifact
approval process, **not** by hashing the unverified download and using that as
its own expected value. HTTPS and SHA-256 do not replace DuckDB's extension
signature verification: leave `SIGIRO_ALLOW_UNSIGNED_EXTENSIONS` unset (its
default is false). Do not invent `.info` metadata files. If signature or version
verification fails, stop and obtain a matching official artifact; do not disable
signature checks to make it load.

## 2. Mount the cache and start once

Transfer `sigiro-extension-cache` intact to the offline host. For the distroless
release image, mount that **directory** at, for example,
`/var/lib/sigiro/extensions` and set
`SIGIRO_EXTENSION_DIR=/var/lib/sigiro/extensions`. Persist a separate writable
`SIGIRO_DATA_DIR` (`/var/lib/sigiro` in the image) for the DuckLake catalog,
Parquet files and temporary files. The container runs as a **nonroot** user:
make both mounts traversable and writable by that user. In particular, the
extension root must be writable even if the pre-seeded file is present: Sigiro
creates it if needed and DuckDB may write extension-cache files. A read-only
mount or a mount over `/var/lib/sigiro` that hides the pre-seeded subdirectory is
not this procedure. Do not rely on a shell or `curl` inside the distroless image.

For a native binary, the equivalent first start is:

```bash
SIGIRO_DATA_DIR=/path/to/writable/data \
SIGIRO_EXTENSION_DIR=/path/to/writable/sigiro-extension-cache \
sigiro serve
```

If `SIGIRO_EXTENSION_DIR` is unset in local mode, Sigiro uses
`<SIGIRO_DATA_DIR>/extensions`; the version/platform directories still go
**under** that root. Set the variable to the cache **root**, not directly to
`v2.0.0-alpha41489/linux_amd64`. Keep the cache persistent across restarts;
a container rebuild alone does not add DuckLake to the image.

## 3. Check storage, not just the listener

Inspect startup logs for `loaded DuckDB extension: ducklake` and
`default project initialized`. Send an OTLP sample to port `4318`, then query
it (allow for the ingest flush interval before expecting a nonzero count):

```bash
curl -fsS -X POST http://127.0.0.1:4318/v1/traces \
  -H 'Content-Type: application/json' \
  --data '{"resourceSpans":[{"resource":{"attributes":[{"key":"service.name","value":{"stringValue":"offline-probe"}}]},"scopeSpans":[{"spans":[{"traceId":"33333333333333333333333333333333","spanId":"4444444444444444","name":"offline-probe","kind":1,"startTimeUnixNano":"1788415000000000000","endTimeUnixNano":"1788415000100000000"}]}]}]}'
curl -fsS -X POST http://127.0.0.1:9999/v1/query \
  -H 'Content-Type: text/plain' \
  --data 'SELECT count(*) AS n FROM lake.sigiro_spans'
```

`/health` returning `ready` on current main **does not prove storage or ingest
works** if default-project initialization failed. The startup fail-before-ready
fix is still under review in [#359](https://github.com/akshatagarwl/sigiro/pull/359);
do not assume it is in your binary. Check the logs and exercise both paths.
A missing extension, wrong version/platform, invalid signature or unwritable
cache is a deployment failure: correct the artifact/mount and restart, rather
than accepting a healthy-looking listener. This guide documents an operator
workaround, not an extension bundled in the image or a fix for DuckDB's proxy
redirects. See [DuckDB's extension installation location](https://duckdb.org/docs/current/extensions/installing_extensions.html)
for the version/platform cache layout.
