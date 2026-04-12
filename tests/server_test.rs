use duckdb::Connection;
use sigiro::server::{validate_read_only, value_ref_to_json};

fn test_conn() -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch("CREATE TABLE test_t (id INTEGER, name VARCHAR, flag BOOLEAN)")
        .expect("create table");
    conn.execute_batch("INSERT INTO test_t VALUES (1, 'hello', true), (2, 'world', false)")
        .expect("insert");
    conn
}

#[test]
fn read_only_via_config_blocks_mutations() {
    let dir = std::env::temp_dir().join("sigiro_test_ro");
    let _removed = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let db_path = dir.join("test_readonly.duckdb");

    {
        let conn = Connection::open(&db_path).expect("open for writing");
        conn.execute_batch("CREATE TABLE test_t (id INTEGER, name VARCHAR)")
            .expect("create");
        conn.execute_batch("INSERT INTO test_t VALUES (1, 'hello')")
            .expect("insert");
    }

    let ro_config = duckdb::Config::default()
        .access_mode(duckdb::AccessMode::ReadOnly)
        .expect("config");
    let ro_conn = Connection::open_with_flags(&db_path, ro_config).expect("open read-only");

    let mut stmt = ro_conn.prepare("SELECT * FROM test_t").unwrap();
    let mut rows = stmt.query([]).unwrap();
    assert!(rows.next().unwrap().is_some());

    let mutations = [
        "DROP TABLE test_t",
        "DELETE FROM test_t",
        "INSERT INTO test_t VALUES (2, 'evil')",
        "UPDATE test_t SET name = 'evil'",
        "CREATE TABLE evil (x INT)",
    ];
    for sql in mutations {
        let err = ro_conn.execute_batch(sql).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("read-only")
                || err.to_string().to_lowercase().contains("read only"),
            "{sql} should fail in read-only mode, got: {err}"
        );
    }

    let _removed = std::fs::remove_dir_all(&dir);
}

#[test]
fn value_ref_integer() {
    let conn = test_conn();
    let mut stmt = conn.prepare("SELECT 42 AS n").unwrap();
    let mut rows = stmt.query([]).unwrap();
    let row = rows.next().unwrap().unwrap();
    assert_eq!(
        value_ref_to_json(row.get_ref(0).unwrap()),
        serde_json::json!(42)
    );
}

#[test]
fn value_ref_text() {
    let conn = test_conn();
    let mut stmt = conn.prepare("SELECT 'hello' AS s").unwrap();
    let mut rows = stmt.query([]).unwrap();
    let row = rows.next().unwrap().unwrap();
    assert_eq!(
        value_ref_to_json(row.get_ref(0).unwrap()),
        serde_json::json!("hello")
    );
}

#[test]
fn value_ref_bool() {
    let conn = test_conn();
    let mut stmt = conn.prepare("SELECT true AS b").unwrap();
    let mut rows = stmt.query([]).unwrap();
    let row = rows.next().unwrap().unwrap();
    assert_eq!(
        value_ref_to_json(row.get_ref(0).unwrap()),
        serde_json::json!(true)
    );
}

#[test]
fn value_ref_null() {
    let conn = test_conn();
    let mut stmt = conn.prepare("SELECT NULL AS n").unwrap();
    let mut rows = stmt.query([]).unwrap();
    let row = rows.next().unwrap().unwrap();
    assert!(value_ref_to_json(row.get_ref(0).unwrap()).is_null());
}

#[test]
fn value_ref_double() {
    let conn = test_conn();
    let mut stmt = conn.prepare("SELECT PI()::DOUBLE AS f").unwrap();
    let mut rows = stmt.query([]).unwrap();
    let row = rows.next().unwrap().unwrap();
    assert_eq!(
        value_ref_to_json(row.get_ref(0).unwrap()),
        serde_json::json!(std::f64::consts::PI)
    );
}

#[test]
fn validate_blocks_dml() {
    assert!(validate_read_only("DROP TABLE sigiro_spans").is_err());
    assert!(validate_read_only("delete from sigiro_spans").is_err());
    assert!(validate_read_only("INSERT INTO sigiro_spans VALUES (1)").is_err());
    assert!(validate_read_only("UPDATE sigiro_spans SET x = 1").is_err());
    assert!(validate_read_only("CREATE TABLE evil (x INT)").is_err());
    assert!(validate_read_only("ATTACH '/tmp/evil.db'").is_err());
    assert!(validate_read_only("SET access_mode = 'read_write'").is_err());
}

#[test]
fn validate_allows_select() {
    assert!(validate_read_only("SELECT * FROM sigiro_spans").is_ok());
    assert!(validate_read_only("SELECT count(*) FROM sigiro_spans WHERE updated_at > 0").is_ok());
    assert!(validate_read_only("SELECT created_by FROM sigiro_spans").is_ok());
}

#[test]
fn validate_blocks_file_access() {
    assert!(validate_read_only("SELECT * FROM read_csv('/etc/passwd')").is_err());
    assert!(validate_read_only("SELECT * FROM read_parquet('data.parquet')").is_err());
    assert!(validate_read_only("SELECT * FROM read_json('data.json')").is_err());
}

#[test]
fn validate_blocks_comment_wrapped() {
    assert!(validate_read_only("SELECT 1 /* DROP TABLE evil */").is_err());
}

#[test]
fn value_ref_mixed_types_from_table() {
    let conn = test_conn();
    let mut stmt = conn
        .prepare("SELECT id, name, flag FROM test_t WHERE id = 1")
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    let row = rows.next().unwrap().unwrap();
    assert_eq!(
        value_ref_to_json(row.get_ref(0).unwrap()),
        serde_json::json!(1)
    );
    assert_eq!(
        value_ref_to_json(row.get_ref(1).unwrap()),
        serde_json::json!("hello")
    );
    assert_eq!(
        value_ref_to_json(row.get_ref(2).unwrap()),
        serde_json::json!(true)
    );
}
