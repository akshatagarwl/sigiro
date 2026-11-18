# Storage schemas are declared and reconciled, never migrated

Every relational store Sigiro owns declares its tables as data: one `CREATE TABLE` statement per table, with `{table}` in place of the name, plus its indexes. That covers ingest cell SQLite, the binary's local `admin.sqlite`, and the hosted PostgreSQL `sigiro_admin` schema. On every start, the store reconciles itself to the declaration in one transaction. A `schema_declaration` table records what each table was built from. A missing table is created. A table whose recorded declaration differs, or that has none yet, is rebuilt with the columns both shapes share. A table recorded but no longer declared is dropped. There are no versioned or hand-written migration steps.

`CREATE TABLE IF NOT EXISTS` was rejected because it silently keeps an old shape. Two tests showed the failure. In an ingest cell upgraded from the previous revision, a new column made every insert fail, and the tenant's ingest answered 503 indefinitely. An older `instance_status` made every admin status write fail on both SQLite and PostgreSQL.

## Consequences

- The reconciler only touches tables it declared or recorded, so runtime-owned tables (Cloudflare `_cf_`, celld `_litestream_`, `sqlite_`) are never at risk.
- A change the old rows cannot satisfy, such as a new `NOT NULL` column without a default on a non-empty table, rolls back and stops the store from opening with the reason. Declare such columns nullable or with a default.
- Rebuilding copies the table, so reshaping a large table costs a full copy at the next start. PostgreSQL serial sequences continue after the copied rows.
- The binary's DuckLake telemetry tables in `src/schema.rs` are outside this rule; ADR-0002 replaces them with segments whose schema the shared crate derives.
