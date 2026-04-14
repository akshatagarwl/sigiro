use duckdb::Connection;
use sigiro::inserter::SignalTable;

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
fn anomalies_view_exists() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create_tables");
    let count: i64 = conn
        .query_row("SELECT count(*) FROM sigiro_anomalies", [], |row| {
            row.get(0)
        })
        .expect("sigiro_anomalies view should be queryable");
    assert_eq!(count, 0);
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
}

#[test]
fn create_tables_with_prefix_idempotent() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch("ATTACH ':memory:' AS lake")
        .expect("attach");
    sigiro::schema::create_tables(&conn, "lake.").expect("first");
    sigiro::schema::create_tables(&conn, "lake.").expect("second should also succeed");
}
