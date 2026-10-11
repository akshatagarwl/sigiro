use sigiro_segments::{validate_read_only, validate_read_only_with};

#[test]
fn a_select_over_a_telemetry_table_is_allowed() {
    assert_eq!(
        validate_read_only("SELECT span_name FROM sigiro_spans WHERE duration > 5"),
        Ok(())
    );
}

#[test]
fn writes_and_attach_are_refused() {
    for sql in [
        "DELETE FROM sigiro_spans",
        "ATTACH 'x.db' AS x",
        "CREATE TABLE t AS SELECT 1",
    ] {
        assert!(validate_read_only(sql).is_err(), "{sql}");
    }
}

#[test]
fn file_readers_and_unknown_tables_are_refused() {
    for sql in [
        "SELECT * FROM read_csv('/etc/passwd')",
        "SELECT * FROM sigiro_anomalies",
        "SELECT getenv('HOME')",
    ] {
        assert!(validate_read_only(sql).is_err(), "{sql}");
    }
}

#[test]
fn a_second_statement_cannot_ride_along() {
    assert_eq!(
        validate_read_only("SELECT 1 FROM sigiro_spans; SELECT 2 FROM sigiro_logs"),
        Err("query rejected: exactly one SQL statement is required, found 2".to_string())
    );
}

#[test]
fn hosted_mode_hides_information_schema() {
    let sql = "SELECT table_name FROM information_schema.tables";
    assert!(validate_read_only_with(sql, true).is_ok());
    assert!(validate_read_only_with(sql, false).is_err());
}
