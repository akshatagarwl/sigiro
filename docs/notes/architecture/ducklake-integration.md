---
title: DuckLake Integration — Storage Layer
created: 2026-03-21
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'competitor', 'data-model', 'decision', 'duckdb']
---

# DuckLake Integration — Storage Layer

**[[plans/STATUS.md|Status]]:** Active — DuckLake is Sigiro's storage layer.
**Updated:** 2026-04-15
**Dependencies:** [[architecture/data-model.md|DuckDB]] with the `ducklake` extension in local mode; `ducklake` + `postgres` in distributed mode; `httpfs` is also loaded for S3 data.

## Storage contract

Sigiro uses DuckLake for table metadata, snapshots, Parquet file management, and
transactions. Each process uses an in-memory [[architecture/data-model.md|DuckDB]]
connection and attaches a DuckLake catalog as `lake`.

There are two explicit storage modes:

| Mode | Catalog | Data files | Selection |
|---|---|---|---|
| Local file (default) | DuckDB-native file catalog, `catalog.ducklake` | Local Parquet under `catalog.ducklake.files/` | `SIGIRO_CATALOG_URL` unset |
| Distributed/hosted | Shared PostgreSQL catalog, with a per-tenant metadata schema | Local Parquet or S3/R2 | `SIGIRO_CATALOG_URL` set |

Local mode is zero-config and does not require PostgreSQL, S3, or the SQLite
extension. S3/R2 configuration is valid only with distributed mode; startup
rejects an S3 bucket configured without `SIGIRO_CATALOG_URL`.

## Local mode

`src/storage.rs::init_default` creates the project directory, installs and loads
`ducklake`, verifies the pinned DuckDB/DuckLake stack, and attaches:

```sql
INSTALL ducklake;
LOAD ducklake;
ATTACH 'ducklake:/path/to/project/catalog.ducklake' AS lake;
```

The catalog is a DuckDB-native `catalog.ducklake` file. DuckLake creates the
associated `catalog.ducklake.files/` directory for local Parquet data.

Each project has an isolated catalog and connection:

```
$SIGIRO_DATA_DIR/
  admin.sqlite                    # administrative state; not telemetry
  extensions/                     # shared DuckDB extension cache
  default/
    catalog.ducklake              # DuckDB-native DuckLake catalog
    catalog.ducklake.files/       # local Parquet telemetry
      main/
        sigiro_spans/...
        sigiro_logs/...
  <other-project>/
    catalog.ducklake
    catalog.ducklake.files/
```

Back up each project's `catalog.ducklake` together with its
`catalog.ducklake.files/` directory. `admin.sqlite` stores administrative state
and is not the DuckLake catalog. The filenames above come from
`src/storage.rs::init_default` and `src/admin.rs::open_admin_db`.

`ProjectPool` creates one [[architecture/data-model.md|DuckDB]] connection per
project and serializes access to that connection. Tables are addressed as
`lake.sigiro_*`; projects do not share an attached catalog.

## Distributed mode

`src/storage.rs::init_distributed` attaches a shared PostgreSQL catalog. The
metadata schema is derived from the tenant ID as `tenant_<tenant_id>`.

With local data files:

```sql
INSTALL ducklake;
LOAD ducklake;
INSTALL postgres;
LOAD postgres;
ATTACH 'ducklake:postgres:<libpq-connection-string>' AS lake
  (METADATA_SCHEMA 'tenant_<tenant_id>',
   DATA_PATH '/path/to/data/tenants/<tenant_id>/',
   CREATE_IF_NOT_EXISTS true);
```

With S3/R2 data, Sigiro also loads `httpfs`, configures an S3 secret, and uses a
path such as `s3://<bucket>/<tenant_id>/` as `DATA_PATH`. The catalog's data
path is pinned when it is created; changing between local and S3 storage
requires an explicit migration/reset rather than silently swapping backends.

Distributed tenant data is laid out below `tenants/` to avoid collisions with
local-mode projects and shared extension files:

```
$SIGIRO_DATA_DIR/
  tenants/
    <tenant_id>/                 # local data when distributed mode uses disk
```

## Write path and appender API

Ingest buffers are flushed through DuckDB's catalog-qualified appender API:

```rust
conn.appender_to_catalog_and_db("sigiro_spans", "lake", "main")
```

The shorter `conn.appender("lake.sigiro_spans")` form is not supported for
these three-part names because DuckDB parses the qualification incorrectly.
DuckLake then commits metadata and writes the Parquet data files. See
[[architecture/write-path.md|Write Path]].

## Operational boundaries

- Local mode is intended for a single Sigiro writer per project directory; the
  pool serializes access within the process.
- PostgreSQL catalog mode provides the shared multi-writer catalog needed for
  distributed/hosted deployments.
- DuckLake owns Parquet file management and compaction. Sigiro does not use a
  second catalog backend or a custom Parquet metadata store.
- Do not expose the catalog file as an ordinary application DuckDB database;
  access it through the DuckLake attachment.

## Known DuckLake constraints

- DuckLake does not support `PRIMARY KEY`, `UNIQUE`, or `CHECK` constraints.
  Schemas must use plain columns and enforce any deduplication in application
  or query logic.
- The standard DuckDB appender must remain the catalog-qualified API described
  above.
- DuckLake pins `DATA_PATH` in the catalog. Treat a local-to-S3 or S3-to-local
  move as a migration, not a runtime configuration toggle.
