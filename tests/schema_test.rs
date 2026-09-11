#![allow(clippy::disallowed_methods)]

use duckdb::Connection;
use sigiro::SignalTable;

fn attach_ducklake(conn: &Connection, dir: &std::path::Path) {
    let data_path = dir.to_string_lossy();
    let extension_dir = std::env::var("SIGIRO_EXTENSION_DIR").unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(|home| format!("{home}/.duckdb/extensions"))
            .unwrap_or_else(|_| "/tmp/sigiro-test-extensions".to_string())
    });
    conn.execute_batch(&format!(
        "SET home_directory='{data_path}'; SET extension_directory='{extension_dir}'; \
         INSTALL ducklake; LOAD ducklake; \
         ATTACH 'ducklake:{data_path}/catalog.ducklake' AS lake"
    ))
    .expect("attach DuckLake");
}

#[test]
fn create_tables_on_fresh_db() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create_tables should succeed");

    for table in SignalTable::ALL {
        let name = table.table_suffix();
        let count: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {name}"), [], |row| {
                row.get(0)
            })
            .unwrap_or_else(|_| panic!("{name} should exist"));
        assert_eq!(count, 0, "{name} should be empty");
    }
}

#[test]
fn create_tables_idempotent() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("first call");
    sigiro::schema::create_tables(&conn, "").expect("second call should also succeed");
}

#[test]
fn anomalies_table_exists_and_accepts_rows() {
    // sigiro_anomalies is a real TABLE (written by the scheduled anomaly
    // pass, read by /v1/diagnose and /v1/anomalies) — not the old
    // threshold-based view.
    let conn = Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create_tables");
    let count: i64 = conn
        .query_row("SELECT count(*) FROM sigiro_anomalies", [], |row| {
            row.get(0)
        })
        .expect("sigiro_anomalies table should be queryable");
    assert_eq!(count, 0);

    // Full-shape insert proves every column of the contract exists.
    conn.execute_batch(
        "INSERT INTO sigiro_anomalies
            (detected_at, service_name, operation, signal, kind, direction,
             ts_shift, \"before\", \"after\", drill_down_sql)
         VALUES (now(), 'svc', 'op', 'error_rate', 'changepoint',
                 'worsening', now(), 1.0, 50.0, 'SELECT 1')",
    )
    .expect("sigiro_anomalies must accept a full anomaly row");
    let count: i64 = conn
        .query_row("SELECT count(*) FROM sigiro_anomalies", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);

    let missing_drill = conn.execute_batch(
        "INSERT INTO sigiro_anomalies
            (detected_at, service_name, operation, signal, kind, direction,
             ts_shift, \"before\", \"after\", drill_down_sql)
         VALUES (now(), 'svc', 'op', 'error_rate', 'changepoint',
                 'worsening', now(), 1.0, 50.0, NULL)",
    );
    assert!(
        missing_drill.is_err(),
        "persisted anomaly shifts require drill_down_sql"
    );
}

#[test]
fn create_tables_with_schema_prefix() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    // Simulate a DuckLake-like setup with an attached database
    conn.execute_batch("ATTACH ':memory:' AS lake")
        .expect("attach in-memory as lake");
    sigiro::schema::create_tables(&conn, "lake.").expect("create tables with prefix");

    for table in SignalTable::ALL {
        let qualified = format!("lake.{}", table.table_suffix());
        let count: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {qualified}"), [], |row| {
                row.get(0)
            })
            .unwrap_or_else(|_| panic!("{qualified} should exist"));
        assert_eq!(count, 0, "{qualified} should be empty");
    }

    // The anomalies table follows the same prefix mechanism as telemetry.
    let count: i64 = conn
        .query_row("SELECT count(*) FROM lake.sigiro_anomalies", [], |row| {
            row.get(0)
        })
        .expect("lake.sigiro_anomalies should exist");
    assert_eq!(count, 0);
}

#[test]
fn create_tables_with_prefix_idempotent() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch("ATTACH ':memory:' AS lake")
        .expect("attach");
    sigiro::schema::create_tables(&conn, "lake.").expect("first");
    sigiro::schema::create_tables(&conn, "lake.").expect("second should also succeed");
}

#[test]
fn unrelated_catalog_schema_does_not_reject_fresh_target() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch(
        "ATTACH ':memory:' AS unrelated;
         CREATE TABLE unrelated.sigiro_logs (
             timestamp TIMESTAMP NOT NULL,
             observed_timestamp BIGINT NOT NULL
         );
         ATTACH ':memory:' AS lake",
    )
    .expect("attach unrelated and target catalogs");

    sigiro::schema::create_tables(&conn, "lake.")
        .expect("an unrelated catalog must not affect fresh target validation");
}

#[test]
fn existing_pre_timestamptz_catalog_is_rejected_loudly() {
    let dir = tempfile::tempdir().expect("temp directory");
    let conn = Connection::open_in_memory().expect("in-memory db");
    attach_ducklake(&conn, dir.path());
    conn.execute_batch(
        "CREATE TABLE lake.sigiro_logs (
            timestamp TIMESTAMP NOT NULL, observed_timestamp BIGINT NOT NULL,
            trace_id VARCHAR, span_id VARCHAR, service_name VARCHAR NOT NULL,
            service_namespace VARCHAR, service_instance_id VARCHAR,
            severity_number INTEGER NOT NULL, severity_text VARCHAR NOT NULL,
            body VARCHAR, resource_attributes VARCHAR, scope_name VARCHAR,
            scope_version VARCHAR, scope_attributes VARCHAR, log_attributes VARCHAR,
            template_id UBIGINT, template_values VARCHAR
         );
         INSERT INTO lake.sigiro_logs
            (timestamp, observed_timestamp, service_name, severity_number, severity_text, body)
         VALUES (make_timestamptz(1), 1, 'old-svc', 9, 'INFO', 'before migration');",
    )
    .expect("create old logs schema");

    let error = sigiro::schema::create_tables(&conn, "lake.")
        .expect_err("pre-TIMESTAMPTZ catalog must be rejected");
    let message = format!("{error:#}");
    assert!(
        message.contains("catalog version is unsupported")
            || message.contains("expected TIMESTAMPTZ")
            || message.contains("partial DuckLake catalog schema"),
        "{message}"
    );
}

#[test]
fn partial_catalog_is_rejected_before_schema_mutation() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch(
        "ATTACH ':memory:' AS lake;
         CREATE TABLE lake.sigiro_logs (
             timestamp TIMESTAMPTZ NOT NULL,
             observed_timestamp TIMESTAMPTZ NOT NULL,
             service_name VARCHAR NOT NULL
         );",
    )
    .expect("create partial catalog");

    let error = sigiro::schema::create_tables(&conn, "lake.")
        .expect_err("partial catalog must be rejected");
    assert!(format!("{error:#}").contains("partial DuckLake catalog schema"));

    let event_name: i64 = conn
        .query_row(
            "SELECT count(*) FROM information_schema.columns
             WHERE table_catalog = 'lake' AND table_name = 'sigiro_logs'
               AND column_name = 'event_name'",
            [],
            |row| row.get(0),
        )
        .expect("inspect partial catalog");
    assert_eq!(event_name, 0, "rejection must not add event_name");
}
