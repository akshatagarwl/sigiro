/// Test whether DuckDB's Appender API works on DuckLake-backed tables.
/// This validates the highest-risk change in the ducklake-default plan (PR #2, Step 4).
/// Uses DuckDB file catalog (not SQLite) — matches production default configuration.
use arrow::array::{Int32Array, StringArray, TimestampMicrosecondArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use duckdb::Connection;
use std::sync::Arc;

fn attach_ducklake_default(conn: &Connection, dir: &std::path::Path) {
    // Point DuckDB's home_directory at the temp dir so INSTALL/LOAD works
    // even in restricted environments (Nix sandbox, CI, containers).
    let data_path = dir.to_string_lossy();
    conn.execute_batch(&format!("SET home_directory='{data_path}';"))
        .expect("set home_directory");

    conn.execute_batch("INSTALL ducklake; LOAD ducklake;")
        .expect("install/load ducklake");

    conn.execute_batch(&format!(
        "ATTACH 'ducklake:{data_path}/catalog.ducklake' AS lake"
    ))
    .expect("attach DuckDB file catalog");
}

fn make_batch(n: usize) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int32, false),
        Field::new("name", DataType::Utf8, true),
        Field::new(
            "ts",
            DataType::Timestamp(TimeUnit::Microsecond, None),
            false,
        ),
    ]));

    let now_us = chrono::Utc::now().timestamp_micros();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int32Array::from((0..n as i32).collect::<Vec<_>>())),
            Arc::new(StringArray::from(vec!["test"; n])),
            Arc::new(TimestampMicrosecondArray::from(
                (0..n).map(|i| now_us + i as i64).collect::<Vec<_>>(),
            )),
        ],
    )
    .expect("valid record batch")
}

#[test]
fn appender_works_on_ducklake_table() {
    let dir = tempfile::tempdir().unwrap();
    let conn = Connection::open_in_memory().unwrap();
    attach_ducklake_default(&conn, dir.path());

    conn.execute_batch("CREATE TABLE lake.test_table (id INTEGER, name VARCHAR, ts TIMESTAMP)")
        .expect("create table");

    // THE TEST: does appender work on a DuckLake-backed table?
    // Must use appender_to_catalog_and_db — conn.appender("lake.table") looks in wrong catalog.
    let mut appender = conn
        .appender_to_catalog_and_db("test_table", "lake", "main")
        .expect("appender should work on DuckLake tables (catalog=lake, schema=main)");

    appender
        .append_record_batch(make_batch(3))
        .expect("append_record_batch should work on DuckLake tables");

    drop(appender);

    let count: i64 = conn
        .query_row("SELECT count(*) FROM lake.test_table", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        count, 3,
        "expected 3 rows after appender flush, got {count}"
    );
}

#[test]
fn appender_multiple_batches_on_ducklake() {
    // Tests the actual inserter.rs pattern: multiple batches flushed sequentially
    let dir = tempfile::tempdir().unwrap();
    let conn = Connection::open_in_memory().unwrap();
    attach_ducklake_default(&conn, dir.path());

    conn.execute_batch("CREATE TABLE lake.test_table (id INTEGER, name VARCHAR, ts TIMESTAMP)")
        .expect("create table");

    {
        let mut appender = conn
            .appender_to_catalog_and_db("test_table", "lake", "main")
            .expect("appender");
        for _ in 0..5 {
            appender
                .append_record_batch(make_batch(100))
                .expect("append_record_batch");
        }
    }

    let count: i64 = conn
        .query_row("SELECT count(*) FROM lake.test_table", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 500, "expected 500 rows, got {count}");
}

#[test]
fn ducklake_data_survives_reattach() {
    // Verify data persists across sessions (catalog.ducklake + Parquet round-trip)
    let dir = tempfile::tempdir().unwrap();
    let data_path = dir.path().to_string_lossy().to_string();

    // Session 1: write data
    {
        let conn = Connection::open_in_memory().unwrap();
        attach_ducklake_default(&conn, dir.path());
        conn.execute_batch("CREATE TABLE lake.test_table (id INTEGER, name VARCHAR, ts TIMESTAMP)")
            .expect("create table");

        let mut appender = conn
            .appender_to_catalog_and_db("test_table", "lake", "main")
            .expect("appender");
        appender
            .append_record_batch(make_batch(42))
            .expect("append");
        drop(appender);
    }

    // Session 2: re-attach and read
    {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!("SET home_directory='{data_path}';"))
            .unwrap();
        conn.execute_batch("INSTALL ducklake; LOAD ducklake;")
            .unwrap();
        conn.execute_batch(&format!(
            "ATTACH 'ducklake:{data_path}/catalog.ducklake' AS lake"
        ))
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT count(*) FROM lake.test_table", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 42, "data should survive re-attach, got {count}");
    }
}

#[test]
fn no_primary_key_on_ducklake_table() {
    // DuckLake does not support PRIMARY KEY — verify it errors or is silently ignored.
    let dir = tempfile::tempdir().unwrap();
    let conn = Connection::open_in_memory().unwrap();
    attach_ducklake_default(&conn, dir.path());

    let result =
        conn.execute_batch("CREATE TABLE lake.test_pk (id UBIGINT, name VARCHAR NOT NULL)");

    // Should succeed — no PK constraint
    assert!(
        result.is_ok(),
        "CREATE TABLE without PK should succeed on DuckLake"
    );

    // Plain INSERT should work (no INSERT OR IGNORE needed)
    conn.execute_batch("INSERT INTO lake.test_pk VALUES (1, 'a')")
        .expect("first insert");
    conn.execute_batch("INSERT INTO lake.test_pk VALUES (1, 'b')")
        .expect("duplicate insert should succeed (no PK enforcement)");

    let count: i64 = conn
        .query_row("SELECT count(*) FROM lake.test_pk", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2, "both rows should exist since no PK enforcement");
}
