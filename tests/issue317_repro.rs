//! Does a DuckLake built from post-fix `main` fix sigiro#317?
//!
//! Shipped for our pinned DuckDB is `eb7b95df82` (2026-07-23), which predates
//! b93bd97d (2026-09-02) and fails every commit against a PostgreSQL catalog.
//!
//! This loads a DuckLake built from post-fix main against our exact DuckDB
//! commit (10de957379) into sigiro's own pinned library — the same static
//! bundle production links — and runs the full write path.
//!
//! Notes learned the hard way:
//!   * `allow_unsigned_extensions` must be set at Config time; it cannot be
//!     changed once the database is running.
//!   * DuckDB derives the extension entrypoint from the FILE NAME, so the file
//!     must be called exactly `ducklake.duckdb_extension`.
//!
//! Needs SIGIRO_TEST_PG and SIGIRO_TEST_DUCKLAKE; skipped when either is unset.

// Selecting the extension under test via the environment IS the point of this
// harness, so the production ban on direct env access does not apply.
#![allow(clippy::disallowed_methods)]

use duckdb::{Config, Connection};

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or_default().to_string()
}

fn open_unsigned() -> Connection {
    let config = Config::default()
        .allow_unsigned_extensions()
        .expect("allow unsigned extensions");
    Connection::open_in_memory_with_flags(config).expect("open duckdb")
}

/// Control: the shipped extension must still fail, so a pass in the other test
/// is attributable to the new binary and nothing else.
#[test]
fn shipped_ducklake_still_fails() {
    let Ok(dsn) = std::env::var("SIGIRO_TEST_PG") else {
        eprintln!("SIGIRO_TEST_PG unset; skipping");
        return;
    };
    let conn = open_unsigned();
    conn.execute_batch("INSTALL postgres; LOAD postgres; INSTALL ducklake; LOAD ducklake;")
        .expect("load shipped extensions");
    let version: String = conn
        .query_row(
            "SELECT coalesce(extension_version,'?') FROM duckdb_extensions() \
             WHERE extension_name='ducklake' AND loaded",
            [],
            |r| r.get(0),
        )
        .expect("version");

    let dir = tempfile::tempdir().expect("tempdir");
    // A unique schema per run: a reused schema pins the DATA_PATH recorded at
    // creation, and a fresh tempdir then trips OVERRIDE_DATA_PATH instead of
    // reaching the commit path this test is about.
    let schema = format!("ctl_{}", std::process::id());
    let attach = format!(
        "ATTACH 'ducklake:postgres:{dsn}' AS lake (METADATA_SCHEMA '{schema}', DATA_PATH '{}/', CREATE_IF_NOT_EXISTS true);",
        dir.path().display()
    );
    conn.execute_batch(&attach).expect("attach");

    let err = conn
        .execute_batch("CREATE TABLE lake.t(a BIGINT);")
        .expect_err("shipped extension must still fail");
    eprintln!("control ({version}): {}", first_line(&err.to_string()));
    assert!(
        err.to_string().contains("multiple commands"),
        "expected the multi-statement rejection, got: {err}"
    );
}

#[test]
fn built_ducklake_commits_to_a_postgres_catalog() {
    let (Ok(dsn), Ok(ext)) = (
        std::env::var("SIGIRO_TEST_PG"),
        std::env::var("SIGIRO_TEST_DUCKLAKE"),
    ) else {
        eprintln!("SIGIRO_TEST_PG / SIGIRO_TEST_DUCKLAKE unset; skipping");
        return;
    };
    let conn = open_unsigned();
    conn.execute_batch("INSTALL postgres; LOAD postgres;")
        .expect("load postgres");
    conn.execute_batch(&format!("LOAD '{ext}';"))
        .unwrap_or_else(|e| panic!("load built extension: {}", first_line(&e.to_string())));

    let version: String = conn
        .query_row(
            "SELECT coalesce(extension_version,'?') FROM duckdb_extensions() \
             WHERE extension_name='ducklake' AND loaded",
            [],
            |r| r.get(0),
        )
        .expect("version");
    eprintln!("built ducklake version = {version} (shipped is eb7b95df82)");

    let dir = tempfile::tempdir().expect("tempdir");
    let schema = format!("built_{}", std::process::id());
    let attach = format!(
        "ATTACH 'ducklake:postgres:{dsn}' AS lake (METADATA_SCHEMA '{schema}', DATA_PATH '{}/', CREATE_IF_NOT_EXISTS true);",
        dir.path().display()
    );
    conn.execute_batch(&attach).expect("attach");

    conn.execute_batch("CREATE TABLE lake.sigiro_spans(a BIGINT, b VARCHAR);")
        .expect("CREATE TABLE must succeed");
    conn.execute_batch("INSERT INTO lake.sigiro_spans VALUES (1,'x'),(2,'y');")
        .expect("first commit");
    conn.execute_batch("INSERT INTO lake.sigiro_spans VALUES (3,'z');")
        .expect("second commit");
    conn.execute_batch("ALTER TABLE lake.sigiro_spans ADD COLUMN c INTEGER;")
        .expect("schema change after data exists");

    let n: i64 = conn
        .query_row("SELECT count(*) FROM lake.sigiro_spans", [], |r| r.get(0))
        .expect("count");
    assert_eq!(n, 3, "rows must round-trip through the parquet data path");
    eprintln!("VERDICT: FIXED — create, two commits, schema change, {n} rows");
}
