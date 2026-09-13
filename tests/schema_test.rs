#![allow(clippy::disallowed_methods)]

use duckdb::Connection;
use sigiro::SignalTable;

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
         VALUES (now()::TIMESTAMP, 'svc', 'op', 'error_rate', 'changepoint',
                 'worsening', now()::TIMESTAMP, 1.0, 50.0, 'SELECT 1')",
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
         VALUES (now()::TIMESTAMP, 'svc', 'op', 'error_rate', 'changepoint',
                 'worsening', now()::TIMESTAMP, 1.0, 50.0, NULL)",
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
