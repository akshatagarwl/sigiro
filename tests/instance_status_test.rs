use rusqlite::Connection;
use sigiro::admin::{create_instance_status_table, read_all_status, write_instance_status};

fn test_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    create_instance_status_table(&conn).unwrap();
    conn
}

#[test]
fn create_table_and_write_keys() {
    let conn = test_conn();
    write_instance_status(&conn, "version", "0.1.0").unwrap();
    write_instance_status(&conn, "mode", "otlp").unwrap();
    write_instance_status(&conn, "platform_os", "linux").unwrap();
    write_instance_status(&conn, "platform_arch", "x86_64").unwrap();
    write_instance_status(&conn, "ebpf_status", "disabled").unwrap();

    let rows = read_all_status(&conn).unwrap();
    assert_eq!(rows.len(), 5);
    assert_eq!(rows.get("version").unwrap(), "0.1.0");
    assert_eq!(rows.get("mode").unwrap(), "otlp");
    assert_eq!(rows.get("ebpf_status").unwrap(), "disabled");
}

#[test]
fn upsert_updates_existing_key() {
    let conn = test_conn();
    write_instance_status(&conn, "version", "0.1.0").unwrap();
    write_instance_status(&conn, "version", "0.2.0").unwrap();

    let rows = read_all_status(&conn).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows.get("version").unwrap(), "0.2.0");
}

#[test]
fn empty_table_returns_empty_map() {
    let conn = test_conn();
    let rows = read_all_status(&conn).unwrap();
    assert!(rows.is_empty());
}
