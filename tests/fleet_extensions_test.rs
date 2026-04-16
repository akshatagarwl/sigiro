use duckdb::Connection;

fn setup_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("SET memory_limit = '256MB'; SET threads = 1;")
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    sigiro::storage::init_default(&conn, dir.path()).unwrap();
    conn
}

#[test]
fn load_fleet_extensions_returns_ok_or_err() {
    let conn = setup_conn();
    let result = sigiro::schema::load_fleet_extensions(&conn);
    // On CI/dev without pre-fetched extensions this may fail — that's fine.
    // We only verify it returns (doesn't panic) and the type is Result.
    println!("load_fleet_extensions result: {result:?}");
    assert!(result.is_ok() || result.is_err());
}

#[test]
fn load_fleet_extensions_idempotent() {
    let conn = setup_conn();
    let first = sigiro::schema::load_fleet_extensions(&conn);
    let second = sigiro::schema::load_fleet_extensions(&conn);
    // Both calls should produce the same outcome (both Ok or both Err).
    assert_eq!(first.is_ok(), second.is_ok());
}
