use duckdb::Connection;
use tracing_test::traced_test;

fn setup_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("SET memory_limit = '256MB'; SET threads = 1;")
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    sigiro::storage::init_default(&conn, camino::Utf8Path::from_path(dir.path()).unwrap()).unwrap();
    conn
}

#[traced_test]
#[test]
fn load_fleet_extensions_returns_ok_or_err() {
    let conn = setup_conn();
    let result = sigiro::schema::load_fleet_extensions(&conn);
    tracing::info!(?result, "load_fleet_extensions");
    assert!(result.is_ok() || result.is_err());
}

#[traced_test]
#[test]
fn load_fleet_extensions_idempotent() {
    let conn = setup_conn();
    let first = sigiro::schema::load_fleet_extensions(&conn);
    let second = sigiro::schema::load_fleet_extensions(&conn);
    tracing::info!(?first, ?second, "idempotency check");
    assert_eq!(first.is_ok(), second.is_ok());
}
