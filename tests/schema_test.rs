use duckdb::Connection;

#[test]
fn create_tables_on_fresh_db() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn).expect("create_tables should succeed");

    let tables = [
        "sigiro_spans",
        "sigiro_logs",
        "sigiro_metrics_gauge",
        "sigiro_metrics_sum",
        "sigiro_metrics_histogram",
        "sigiro_metrics_exp_histogram",
    ];
    for table in tables {
        let count: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap_or_else(|_| panic!("{table} should exist"));
        assert_eq!(count, 0, "{table} should be empty");
    }
}

#[test]
fn create_tables_idempotent() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn).expect("first call");
    sigiro::schema::create_tables(&conn).expect("second call should also succeed");
}

#[test]
fn anomalies_view_exists() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn).expect("create_tables");
    let count: i64 = conn
        .query_row("SELECT count(*) FROM sigiro_anomalies", [], |row| {
            row.get(0)
        })
        .expect("sigiro_anomalies view should be queryable");
    assert_eq!(count, 0);
}
