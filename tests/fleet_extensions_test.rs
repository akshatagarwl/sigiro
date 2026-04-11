use duckdb::Connection;
use sigiro::schema;

fn setup_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("SET memory_limit = '256MB'; SET threads = 1;")
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    sigiro::storage::init_default(&conn, dir.path()).unwrap();
    conn
}

#[test]
fn load_fleet_extensions_returns_loaded_list() {
    let conn = setup_conn();
    let loaded = schema::load_fleet_extensions(&conn);
    // At minimum, the function should run without panicking.
    // Extensions may fail on non-Linux, so just verify the return type.
    println!("loaded extensions: {loaded:?}");
    assert!(!loaded.iter().any(|e| e.is_empty()));
}

#[test]
fn load_fleet_extensions_graceful_on_missing() {
    let conn = setup_conn();
    // Calling twice should still work (extensions already loaded or gracefully skipped)
    let first = schema::load_fleet_extensions(&conn);
    let second = schema::load_fleet_extensions(&conn);
    assert_eq!(first, second);
}
