---
title: "Plan: SQLite Catalog Migration"
created: 2026-04-10
updated: 2026-04-15
type: summary
tags: ['ai-agent', 'architecture', 'auth', 'competitor', 'data-model', 'duckdb']
---

# Plan: SQLite Catalog Migration

**Date:** 2026-04-09
**[[plans/STATUS.md|Status]]:** In Progress — ~90% done
**Note:** Core migration is done (SQLite catalog, ducklake + sqlite extensions). Remaining cleanup: delete `StorageConfig` + `load_config` from `src/storage.rs`, delete `init_ducklake` alias, and update `src/pool.rs` to remove `load_config` branching.

## Goal

Unify sigiro's storage layer into a single code path using [[architecture/ducklake-integration.md|DuckLake]] with SQLite as the catalog.
Eliminate the [[architecture/data-model.md|DuckDB]] file catalog vs PostgreSQL catalog branching. Enable FUSE-mounted object
storage (rclone + B2/R2) for cloud mode without requiring PostgreSQL.

## Why

1. [[architecture/data-model.md|DuckDB]] file catalog has locking issues on FUSE mounts (rclone, S3 Files, etc.)
2. SQLite handles file locking properly on network/FUSE filesystems
3. PostgreSQL catalog adds $15-30/mo [[company/cost.md|cost]] and operational complexity
4. Two code paths (`init_default` + `init_ducklake`) is unnecessary — both use [[architecture/ducklake-integration.md|DuckLake]]
5. Single code path = less maintenance, fewer bugs

## Changes

### 1. `src/storage.rs` — unified init

- Delete `StorageConfig` struct and `load_config()` function
- Delete `init_ducklake()` (PostgreSQL + S3 path)
- Rename `init_default()` → `init()`, change catalog from [[architecture/data-model.md|DuckDB]] file to SQLite
- Extensions: `ducklake` + `sqlite` (was just `ducklake`)
- ATTACH: `ducklake:sqlite:{dir}/catalog.sqlite` (was `ducklake:{dir}/catalog.ducklake`)
- Remove `envy` and `serde::Deserialize` imports

### 2. `src/pool.rs` — remove branching

- Remove `match crate::storage::load_config()` block
- Just call `crate::storage::init(&conn, &project_dir)?`

### 3. `Cargo.toml` — remove envy

- Remove `envy = "0.4"` (only used for StorageConfig)

### 4. Docs updates

- `docs/architecture/ducklake-integration.md` — rewrite catalog strategy, file layout
- `docs/architecture/write-path.md` — remove "local mode" vs "[[architecture/ducklake-integration.md|DuckLake]] mode" distinction
- `docs/getting-started.md` — simplify examples, remove S3 env vars from default

## File Layout (after)

```
~/.sigiro/data/
  admin.sqlite                          ← API keys (changed from admin.duckdb)
  {project_id}/
    catalog.sqlite                      ← DuckLake catalog (SQLite, not DuckDB)
    catalog.ducklake.files/                         ← Parquet data (auto-created by DuckLake)
      main/
        sigiro_spans/...
        sigiro_logs/...
```

## Cloud Mode

```bash
# Mount B2/R2 via rclone
rclone mount b2:sigiro-tenants /mnt/sigiro

# Point sigiro at the mount
SIGIRO_DATA_DIR=/mnt/sigiro cargo run -- serve
```

Same code. [[architecture/data-model.md|DuckDB]] reads "local" files. SQLite handles locking on FUSE.
No PostgreSQL, no httpfs, no S3 secrets.

## Testing

1. `cargo run -- serve` — should start with local SQLite catalog
2. Start a local Open server and verify it accepts ingest without a bearer key
3. Insert test data via OTLP, verify [[architecture/data-model.md|Parquet]] files created
4. Compaction should still work (ducklake_merge_adjacent_files is catalog-agnostic)
