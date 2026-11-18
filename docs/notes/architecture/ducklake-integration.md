---
title: DuckLake Integration — Storage Layer
created: 2026-03-21
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'competitor', 'data-model', 'decision', 'duckdb']
---

# DuckLake Integration — Storage Layer

**[[plans/STATUS.md|Status]]:** Active — DuckLake is sigiro's storage layer. SQLite catalog by default.
**Date:** 2026-03-21
**Updated:** 2026-04-09 — SQLite catalog as default (single code path, no PostgreSQL)
**Deps:** [[architecture/data-model.md|DuckDB]] `ducklake` extension + `sqlite` extension (always loaded) + `postgres` extension (distributed mode only)

---

## Decision

**DuckLake** replaces both the current [[architecture/data-model.md|DuckDB]]-as-buffer approach AND the proposed [[architecture/data-model.md|Parquet]]-native
custom code. It handles [[architecture/data-model.md|Parquet]] file management, compaction, ACID transactions, multi-writer
coordination, and S3 cold tier — all things we would otherwise build from scratch (~200+ LOC)
with our own undiscovered bugs.

**SQLite catalog** is the default — `ATTACH 'ducklake:sqlite:{dir}/catalog.sqlite' AS lake`.
Zero extra services, zero config, works on FUSE mounts (rclone, S3 Files). SQLite handles
file locking properly on network filesystems, unlike [[architecture/data-model.md|DuckDB]] file catalog which has conflicts
on FUSE.

**Single code path** — no environment variables, no branching. Same code works on local disk
and FUSE-mounted object storage (B2, R2, S3).

### File-based catalog strategy

|| Tier | Catalog | Storage | Trigger | Use case ||
||------|---------|---------|---------|----------|
|| **Default** | SQLite | Local Parquet on volume or FUSE mount | No env vars needed | Self-hosted, Fly.io, bare metal |
|| **Distributed** | PostgreSQL | S3 | `SIGIRO_CATALOG_URL` set | Multi-node, SaaS |

Migration between tiers: **copy [[architecture/data-model.md|Parquet]] files to S3, ATTACH with PostgreSQL catalog.** The catalog
is ephemeral metadata — it can be rebuilt from [[architecture/data-model.md|Parquet]] files. No export/import tooling needed.

### Per-project isolation

Each project gets its own `catalog.sqlite` + [[architecture/data-model.md|Parquet]] directory:

```
~/.sigiro/data/
  project_abc123/
    catalog.sqlite              # SQLite with DuckLake schema
    catalog.ducklake.files/                # Parquet data (auto-created by DuckLake)
      main/
        sigiro_spans/...
        sigiro_logs/...
  project_def456/
    catalog.sqlite
    catalog.ducklake.files/
      main/
        sigiro_spans/...
```

`pool.rs` creates one [[architecture/data-model.md|DuckDB]] connection per project, each ATTACHing to its own `catalog.sqlite`.
No cross-project data access. Same concurrency model as before — `Arc<Mutex<Connection>>` per project.

## What DuckLake gives us (vs building from scratch)

| Feature | DuckLake | Custom Parquet-native |
|---------|----------|----------------------|
| Parquet file management | Built-in | ~80 LOC (incoming/ + data/ tiers) |
| Compaction | Built-in (`ducklake_merge_adjacent_files`) | ~50 LOC (merge small files) |
| ACID transactions | Built-in (catalog-coordinated) | Manual (file rename atomicity) |
| Multi-writer (distributed) | Built-in (PostgreSQL optimistic locking) | Not supported |
| S3 upload | Built-in | ~30 LOC (opendal) |
| Schema evolution | Built-in | Manual |
| DELETE for retention | Built-in | Manual (file deletion) |
| Crash recovery | Built-in (catalog is source of truth) | Manual (watermark table) |

## Known Bugs That Affect Us

### #867 [MEDIUM]: Wrong stats on varchar filter with null min/max

**Impact on sigiro:** Our tables have VARCHAR columns (service_name, resource_attributes,
body, severity_text). If column stats have null min/max (which happens when a column has
only NULL values in some files), DuckLake may use wrong stats from another table's column
with the same column_id, causing queries to return empty results.

**Our columns at risk:** resource_attributes (can have NULLs), severity_text (can have NULLs).

**Workaround:** `SET disabled_optimizers = 'filter_pushdown'` (confirmed by reporter).
But this kills query performance. Better: ensure our columns never have all-NULL files
(sigiro always populates service_name, timestamp — the primary filter columns).

**Instrumentation to detect:**
```rust
/// Verify query results are not silently empty due to DuckLake stats bug #867.
/// Run after every investigation query that filters on VARCHAR columns.
/// If the raw count > 0 but filtered count = 0, the stats bug may be active.
fn verify_not_empty_due_to_stats_bug(
    conn: &Connection,
    table: &str,
    service: &str,
    from_ts: i64,
    to_ts: i64,
) -> Result<()> {
    // Count without varchar filter
    let raw_count: i64 = conn.query_row(
        &format!("SELECT count(*) FROM {table} WHERE timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)"),
        duckdb::params![from_ts, to_ts],
        |row| row.get(0),
    )?;
    // Count with varchar filter
    let filtered_count: i64 = conn.query_row(
        &format!("SELECT count(*) FROM {table} WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)"),
        duckdb::params![service, from_ts, to_ts],
        |row| row.get(0),
    )?;
    if raw_count > 0 && filtered_count == 0 {
        tracing::error!(
            table,
            service,
            raw_count,
            "DuckLake stats bug #867 suspected: raw data exists but filter returned 0"
        );
        // Workaround: rerun query with SET disabled_optimizers = 'filter_pushdown'
    }
    Ok(())
}
```

**Goal:** Detect in production, collect reproduction data, upstream a fix.

### #862 [LOW]: COPY from ducklake table produces UTF8 error

**Impact on sigiro:** Would affect us if we COPY FROM ducklake to [[architecture/data-model.md|Parquet]] (e.g., for export).
We don't do this — DuckLake manages its own [[architecture/data-model.md|Parquet]] files internally.

**Instrumentation:** None needed. We don't use this code path.

### #866 [LOW]: Compaction crashes with per_thread_output

**Impact on sigiro:** If we enable `per_thread_output` in DuckLake config, compaction fails.
Fix is already submitted as PR (#866). Don't enable `per_thread_output` until merged.

**Instrumentation:**
```rust
/// Log compaction results. If compaction fails, log the error and continue.
/// DuckLake compaction is best-effort — query correctness doesn't depend on it.
fn run_ducklake_compaction(conn: &Connection, table: &str) -> Result<()> {
    match conn.execute_batch(&format!("CALL ducklake_merge_adjacent_files('{table}')")) {
        Ok(_) => tracing::info!(table, "ducklake compaction complete"),
        Err(e) => tracing::error!(table, error = %e, "ducklake compaction failed (non-fatal, #866?)"),
    }
    Ok(())
}
```

### #865 [NONE]: UPDATE produces wrong results

**Impact on sigiro:** We never UPDATE. Append-only INSERT + DELETE for retention.
Does not affect us. No instrumentation needed.

## Bugs That Do NOT Affect Us

| Bug | Why safe |
|-----|---------|
| #865 (UPDATE wrong results) | We never UPDATE — append-only + DELETE |
| #869 (O(n²) on large UPDATE) | We never UPDATE |
| #836 (GCS batch delete fails) | We use S3, not GCS |
| #871 (path separator on relative paths) | We use absolute paths |

## Architecture with DuckLake

```
OTLP gRPC/HTTP
    |
    v
otlp2records (Arrow RecordBatch)
    |
    v
DuckDB INSERT INTO lake.sigiro_spans (via appender)
    |   appender_to_catalog_and_db("table", "lake", "main")
    v
DuckLake manages Parquet files automatically:
  - Writes to local disk or S3
  - Compaction (merge adjacent files)
  - Catalog tracks metadata in DuckDB file (default) or PostgreSQL (distributed)
    |
    v
Queries: SELECT * FROM lake.sigiro_spans WHERE ...
    |
    v
/diagnose --> InvestigationBlock
```

### Per-project data flow

```
pool.get_or_create("project_abc123")
  → opens ~/.sigiro/data/project_abc123/catalog.sqlite
yc|  → ATTACH 'ducklake:sqlite:catalog.sqlite' AS lake
  → CREATE TABLE IF NOT EXISTS lake.sigiro_spans (...)
  → returns Arc<Mutex<Connection>>

inserter flushes batches via:
  → conn.appender_to_catalog_and_db("sigiro_spans", "lake", "main")
  → DuckLake writes Parquet to catalog.ducklake.files/
```

### Setup — Default (SQLite catalog + local Parquet)

```sql
INSTALL ducklake; LOAD ducklake;
INSTALL sqlite; LOAD sqlite;

-- Zero config — catalog is SQLite, data is local Parquet
ke|-- Zero config — catalog is SQLite, data is local Parquet (DuckLake default: catalog.ducklake.files/)
ke|ATTACH 'ducklake:sqlite:catalog.sqlite' AS lake;

-- Create tables (DuckLake manages Parquet files)
CREATE TABLE lake.sigiro_spans (...);
CREATE TABLE lake.sigiro_logs (...);
CREATE TABLE lake.sigiro_metrics_gauge (...);
-- etc.
```

### Setup — Distributed (PostgreSQL catalog + S3)

```sql
INSTALL ducklake; LOAD ducklake;
INSTALL postgres; LOAD postgres;

-- PostgreSQL catalog + S3 data
ATTACH 'ducklake:postgres:host=localhost dbname=sigiro_catalog'
    AS lake (DATA_PATH 's3://bucket/sigiro/');
```

### Appender API — verified

The standard `conn.appender("lake.sigiro_spans")` **fails** — [[architecture/data-model.md|DuckDB]]'s appender parses
the dot-separated name as `catalog.schema.table` incorrectly, producing:
```
Table .main.lake.sigiro_spans could not be found
```

**Must use** the catalog-qualified API:
```rust
// WRONG — dot-qualified name fails
conn.appender("lake.sigiro_spans")

// CORRECT — explicit catalog + schema
conn.appender_to_catalog_and_db("sigiro_spans", "lake", "main")
```

This calls `duckdb_appender_create_ext()` under the hood with separate catalog/schema/table
parameters. Verified at 374k rows/sec batched throughput.

### PRIMARY KEY — not supported

DuckLake does not support PRIMARY KEY, UNIQUE, or CHECK constraints:
```
Not implemented Error: PRIMARY KEY/UNIQUE constraints are not supported in DuckLake
```

`sigiro_log_templates` currently has `template_id UBIGINT PRIMARY KEY`. Must change to
plain `template_id UBIGINT`. Deduplication handled at query time or via compactor cleanup.

## Migration Between Tiers

The catalog is **ephemeral metadata** — schema versions, snapshot IDs, file registry.
The actual data is [[architecture/data-model.md|Parquet]], which is universal. Migration is:

1. **Copy [[architecture/data-model.md|Parquet]] files to S3** (or leave them — DuckLake can read from multiple paths)
2. **ATTACH with PostgreSQL catalog** pointing at S3 `DATA_PATH`
3. **Optionally** run `CALL ducklake_register_files('lake', 's3://bucket/**/*.parquet')` to
   register existing files in the new catalog
4. Done — same tables, same queries, zero code change

No export/import tools needed. No catalog-specific migration. The [[architecture/data-model.md|Parquet]] files are the
source of truth; the catalog just tracks where they are.

## Implementation Plan

### Phase 1: Default to DuckLake — DONE (PR #2)

[[architecture/data-model.md|DuckDB]] file catalog is now the default. No local mode. All tables use `lake.*` prefix.

### Phase 2: Background compaction + cleanup (post-merge)

- Add periodic `ducklake_merge_adjacent_files()` via tokio background task
- Add periodic `ducklake_cleanup_old_files()` for retention
- Add `DELETE FROM lake.sigiro_spans WHERE timestamp < now() - INTERVAL '7 DAY'` for retention

### Phase 3: S3 migration

When data outgrows volume:
1. Set `SIGIRO_CATALOG_URL` (PostgreSQL connection string)
2. Set `SIGIRO_S3_BUCKET` + `SIGIRO_S3_REGION`
3. Restart sigiro — ATTACH uses PostgreSQL catalog + S3 DATA_PATH
4. Optionally copy existing local [[architecture/data-model.md|Parquet]] files to S3 and register via `ducklake_register_files`

## Deps Removed (vs Parquet-native plan)

- `parquet` crate — DuckLake writes [[architecture/data-model.md|Parquet]] internally
- `opendal` — DuckLake handles S3 via httpfs
- Custom compaction code (~50 LOC)
- Custom file management code (~80 LOC)
- Custom incoming/data tier logic (~40 LOC)

## Deps Added

- [[architecture/data-model.md|DuckDB]] `ducklake` extension (default — always needed)
- [[architecture/data-model.md|DuckDB]] `sqlite` extension (catalog backend — always needed)
- [[architecture/data-model.md|DuckDB]] `postgres` extension (distributed mode — loaded when `SIGIRO_CATALOG_URL` is set)

## Deps Removed (vs PostgreSQL catalog plan)

- PostgreSQL catalog branching code (~80 LOC in pool.rs, main.rs)
- `envy` dependency for config loading
- `StorageConfig` struct and related logic

## Contributing Back

When we hit a DuckLake bug in production:
1. Instrumentation detects it (tracing::error with context)
2. Collect reproduction data (table schema, row counts, query, expected vs actual)
3. File issue on github.com/duckdb/ducklake with minimal reproduction
4. If fix is obvious from reading source: submit PR
5. If blocking: apply workaround (e.g., disabled_optimizers) and document in code comment
