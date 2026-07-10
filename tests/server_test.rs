use duckdb::Connection;
use sigiro::server::{duckdb_value_to_json, validate_read_only};

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
        duckdb_value_to_json(row.get_ref(0).unwrap()),
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
        duckdb_value_to_json(row.get_ref(0).unwrap()),
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
        duckdb_value_to_json(row.get_ref(0).unwrap()),
        serde_json::json!(true)
    );
}

#[test]
fn value_ref_null() {
    let conn = test_conn();
    let mut stmt = conn.prepare("SELECT NULL AS n").unwrap();
    let mut rows = stmt.query([]).unwrap();
    let row = rows.next().unwrap().unwrap();
    assert!(duckdb_value_to_json(row.get_ref(0).unwrap()).is_null());
}

#[test]
fn value_ref_double() {
    let conn = test_conn();
    let mut stmt = conn.prepare("SELECT PI()::DOUBLE AS f").unwrap();
    let mut rows = stmt.query([]).unwrap();
    let row = rows.next().unwrap().unwrap();
    assert_eq!(
        duckdb_value_to_json(row.get_ref(0).unwrap()),
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
    // The wider file-reader family the SQL guard also denies (defense-in-depth
    // alongside the engine-level filesystem lockdown).
    assert!(validate_read_only("SELECT * FROM read_text('/etc/passwd')").is_err());
    assert!(validate_read_only("SELECT * FROM read_blob('/etc/passwd')").is_err());
    assert!(validate_read_only("SELECT * FROM read_csv_auto('/etc/passwd')").is_err());
    assert!(validate_read_only("SELECT * FROM parquet_scan('x.parquet')").is_err());
    assert!(validate_read_only("SELECT * FROM read_json_auto('x.json')").is_err());
    assert!(validate_read_only("SELECT * FROM read_ndjson_auto('x.json')").is_err());
    assert!(validate_read_only("SELECT file FROM glob('/etc/*')").is_err());
}

#[test]
fn validate_blocks_comment_wrapped() {
    // DROP inside a comment is a harmless SELECT; the real DROP guard is the
    // SELECT-only AST check, not the tokenizer word scan.
    assert!(validate_read_only("SELECT 1 /* DROP TABLE evil */").is_ok());
}

#[test]
fn validate_blocks_file_access_hidden_by_string_literal() {
    // The confirmed bypass: substring-scanning after naive comment-stripping
    // let `--` inside a string literal truncate the scan before `read_csv`.
    // The tokenizer scans the real token stream, so it isn't fooled.
    assert!(validate_read_only("SELECT '--' AS c, t.* FROM read_csv('/etc/passwd') AS t").is_err());
    assert!(validate_read_only("SELECT '/*' AS c, t.* FROM read_parquet('x') AS t").is_err());
    assert!(validate_read_only("SELECT '/* */', t.* FROM read_json('x') AS t").is_err());
}

#[test]
fn validate_allows_blocked_words_inside_string_literals() {
    // False-positive gone: a blocked keyword appearing only inside a string
    // literal (not as an identifier/table-function token) must not trip the
    // guard.
    assert!(
        validate_read_only("SELECT 'read_csv is a function' AS note FROM sigiro_spans").is_ok()
    );
}

#[test]
fn validate_allows_ducklake_time_travel() {
    // DuckLake time-travel reads (the `=>` form sqlparser cannot parse) must
    // be accepted — they are first-class debugging reads for this system.
    assert!(validate_read_only("SELECT count(*) FROM lake.sigiro_logs AT (VERSION => 3)").is_ok());
    assert!(
        validate_read_only(
            "SELECT * FROM lake.sigiro_logs AT (TIMESTAMP => '2026-07-09 19:55:00') \
             WHERE service_name = 'x'"
        )
        .is_ok()
    );
}

#[test]
fn validate_time_travel_does_not_bypass_guards() {
    // Stripping the AT clause for parsing must not become a hole in the
    // write/DDL/file-access guards, which still scan the original SQL.
    assert!(validate_read_only("SELECT * FROM read_csv('/etc/passwd') AT (VERSION => 1)").is_err());
    assert!(validate_read_only("DROP TABLE sigiro_logs AT (VERSION => 1)").is_err());
}

// ---------------------------------------------------------------------------
// M1: AST allowlist — closes the replacement-scan hole (a string-literal FROM
// target parses as a plain relation name, not a Word token, so the old
// tokenizer-only denylist never saw it). See ALLOWED_TABLES/SAFE_FUNCTIONS in
// src/server.rs for the exact sourced sets.
// ---------------------------------------------------------------------------

#[test]
fn validate_blocks_replacement_scan_string_literals() {
    assert!(validate_read_only("SELECT * FROM 's3://b/x.parquet'").is_err());
    assert!(validate_read_only("SELECT * FROM '/etc/passwd'").is_err());
    assert!(validate_read_only("SELECT * FROM 'https://x/y'").is_err());
}

#[test]
fn validate_blocks_unsafe_function_in_subquery() {
    assert!(validate_read_only("SELECT * FROM (SELECT * FROM read_csv('/etc/passwd'))").is_err());
}

#[test]
fn validate_blocks_unsafe_function_in_cte() {
    assert!(
        validate_read_only("WITH e AS (SELECT * FROM read_parquet('x')) SELECT * FROM e").is_err()
    );
}

#[test]
fn validate_blocks_unsafe_function_in_union() {
    assert!(validate_read_only("SELECT 1 UNION SELECT * FROM read_text('/etc/passwd')").is_err());
}

#[test]
fn validate_allows_allowlisted_tables() {
    assert!(validate_read_only("SELECT count(*) FROM sigiro_spans").is_ok());
    assert!(validate_read_only("SELECT * FROM lake.sigiro_logs WHERE service_name='x'").is_ok());
}

#[test]
fn validate_allows_safe_functions_actually_used_by_sigiro() {
    // src/diagnose.rs `check_duckdb_health` — DuckDB memory self-check.
    assert!(
        validate_read_only(
            "SELECT memory_limit, memory_usage FROM pragma_database_size() WHERE database_name = 'lake'"
        )
        .is_ok()
    );
    // tests/e2e/docker/run.sh — schema-detection query against /v1/query.
    assert!(
        validate_read_only(
            "SELECT table_schema FROM information_schema.tables WHERE table_name = 'sigiro_profiles' LIMIT 1"
        )
        .is_ok()
    );
}

#[test]
fn validate_blocks_unknown_relation_even_when_bare() {
    // A bare identifier that simply isn't a sigiro table/view must still be
    // rejected — the allowlist is positive, not a denylist.
    assert!(validate_read_only("SELECT * FROM some_other_table").is_err());
}

#[test]
fn validate_blocks_unrecognized_schema_qualifier() {
    // A recognized table name under an unrecognized schema/catalog qualifier
    // must not slip through on the final-identifier match alone.
    assert!(validate_read_only("SELECT * FROM evil_schema.sigiro_spans").is_err());
}

// ---------------------------------------------------------------------------
// CTE (`WITH`) policy: REJECTED outright. CTE-name allowlisting was abandoned
// as too fragile a security boundary after two subtle bypasses were found —
// a cross-scope leak (a nested scalar subquery's `WITH evil AS (...)` name
// leaking into an unrelated outer physical `FROM evil`, because sqlparser
// visits a SELECT's projection before its FROM) and a forward reference (a
// later CTE named `evil` failing to shadow the physical `evil` table read by
// an earlier CTE). `validate_read_only` now rejects ANY query with a `WITH`
// clause at any depth; subqueries cover the same legitimate use cases.
// ---------------------------------------------------------------------------

#[test]
fn validate_blocks_cte_scope_leak_into_outer_from() {
    // Former cross-scope-leak bypass: a scalar subquery in the projection
    // list defines `WITH evil AS (...)`; the outer FROM then reads a REAL,
    // non-allowlisted table also named `evil`. Still rejected — now simply
    // because it uses `WITH` at all.
    assert!(
        validate_read_only("SELECT (WITH evil AS (SELECT 1) SELECT 1), secret FROM evil").is_err()
    );
}

#[test]
fn validate_blocks_cte_forward_reference_to_physical_table() {
    // Former forward-reference bypass: a later CTE named `evil` does not
    // shadow the physical `evil` table that the earlier CTE `a` reads via
    // `FROM evil`. Still rejected — now because `WITH` is banned outright.
    assert!(
        validate_read_only("WITH a AS (SELECT * FROM evil), evil AS (SELECT 1) SELECT * FROM a")
            .is_err()
    );
}

#[test]
fn validate_blocks_cte_scope_leak_deeper_nesting() {
    // Deeper nesting of the former leak bypass — all rejected because they
    // use `WITH` anywhere in the tree (nested scalar subquery, sibling
    // projection item, FROM-list sibling).
    assert!(
        validate_read_only(
            "SELECT (SELECT (WITH evil AS (SELECT 1) SELECT 1) FROM sigiro_spans LIMIT 1), secret FROM evil"
        )
        .is_err()
    );
    assert!(
        validate_read_only(
            "SELECT (WITH evil AS (SELECT 1) SELECT 1) AS a, (SELECT 1 FROM evil) AS b FROM sigiro_spans"
        )
        .is_err()
    );
    assert!(
        validate_read_only(
            "SELECT 1 FROM (SELECT (WITH evil AS (SELECT 1) SELECT 1) AS x) t, evil"
        )
        .is_err()
    );
}

#[test]
fn validate_rejects_all_cte_usage_by_policy() {
    // CTEs are disallowed by policy (see the section comment): even a plain,
    // fully-allowlisted `WITH` query is now REJECTED — CTE-name allowlisting
    // was too fragile a security boundary, so `WITH` is banned outright.
    // (Previously `validate_allows_legit_cte_usage`, now flipped.)
    assert!(
        validate_read_only(
            "WITH x AS (SELECT service_name FROM sigiro_spans WHERE timestamp > now() - interval 1 hour) SELECT count(*) FROM x"
        )
        .is_err()
    );
    // A CTE referencing an earlier CTE in the same WITH clause — also rejected.
    assert!(
        validate_read_only(
            "WITH a AS (SELECT service_name FROM sigiro_spans), b AS (SELECT * FROM a) SELECT * FROM b"
        )
        .is_err()
    );
    // A CTE reusing an allowlisted table name — also rejected (no more
    // shadowing exception). (Previously
    // `validate_allows_cte_shadowing_allowlisted_table_name`, now flipped.)
    assert!(
        validate_read_only("WITH sigiro_spans AS (SELECT 1 AS x) SELECT * FROM sigiro_spans")
            .is_err()
    );
}

#[test]
fn validate_allows_subquery_forms_that_replace_ctes() {
    // Subqueries are the sanctioned replacement for CTEs and remain fully
    // allowed — this is what keeps legitimate read patterns working under
    // the WITH-ban.
    assert!(validate_read_only("SELECT * FROM (SELECT service_name FROM sigiro_spans) t").is_ok());
    assert!(
        validate_read_only(
            "SELECT count(*) FROM sigiro_spans WHERE timestamp > now() - interval 1 hour"
        )
        .is_ok()
    );
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
        duckdb_value_to_json(row.get_ref(0).unwrap()),
        serde_json::json!(1)
    );
    assert_eq!(
        duckdb_value_to_json(row.get_ref(1).unwrap()),
        serde_json::json!("hello")
    );
    assert_eq!(
        duckdb_value_to_json(row.get_ref(2).unwrap()),
        serde_json::json!(true)
    );
}
