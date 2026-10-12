use std::collections::BTreeMap;

use sigiro::admin::{
    open_hosted_admin_db, open_local_admin_db, read_all_status, write_instance_status,
};

fn statuses(conn: &mut sigiro::admin::AdminConnection) -> BTreeMap<String, String> {
    read_all_status(conn).unwrap().into_iter().collect()
}

#[test]
fn an_older_local_admin_database_is_reshaped_and_keeps_its_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
    {
        let old = rusqlite::Connection::open(path.join("admin.sqlite")).unwrap();
        old.execute_batch(
            "CREATE TABLE instance_status (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO instance_status VALUES ('version', '0.1.0');
             CREATE TABLE schema_declaration (table_name TEXT PRIMARY KEY, declaration TEXT NOT NULL);
             CREATE TABLE retired_table (x TEXT);
             INSERT INTO schema_declaration VALUES ('retired_table', 'CREATE TABLE retired_table (x TEXT)');",
        )
        .unwrap();
    }

    let mut conn = open_local_admin_db(&path).unwrap();
    write_instance_status(&mut conn, "mode", "local").unwrap();
    assert_eq!(
        statuses(&mut conn),
        BTreeMap::from([
            ("mode".into(), "local".into()),
            ("version".into(), "0.1.0".into())
        ])
    );
    drop(conn);

    let check = rusqlite::Connection::open(path.join("admin.sqlite")).unwrap();
    let retired: i64 = check
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'retired_table'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        retired, 0,
        "an undeclared table this database once declared is dropped"
    );

    let mut reopened = open_local_admin_db(&path).unwrap();
    assert_eq!(
        statuses(&mut reopened).len(),
        2,
        "reopening an up-to-date database keeps every row"
    );
}

#[tokio::test]
async fn an_older_hosted_admin_schema_is_reshaped_and_keeps_its_rows() {
    #[expect(
        clippy::disallowed_methods,
        reason = "selects an external test database, not application configuration"
    )]
    let Ok(url) = std::env::var("SIGIRO_TEST_POSTGRES_URL") else {
        eprintln!(
            "skipped: set SIGIRO_TEST_POSTGRES_URL (scripts/test-admin-postgres.sh) to run against PostgreSQL"
        );
        return;
    };
    let setup = url.clone();
    tokio::task::spawn_blocking(move || {
        let mut old = sigiro::admin::connect_postgres(&setup).unwrap();
        old.batch_execute(
            "DROP SCHEMA IF EXISTS sigiro_admin CASCADE;
             CREATE SCHEMA sigiro_admin;
             CREATE TABLE sigiro_admin.instance_status (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO sigiro_admin.instance_status VALUES ('version', '0.1.0');
             CREATE TABLE sigiro_admin.alert_outbox (id BIGSERIAL PRIMARY KEY, project_id TEXT NOT NULL, fingerprint TEXT NOT NULL,
               transition TEXT NOT NULL, payload_json TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0,
               next_attempt_at_us BIGINT NOT NULL, sent_at_us BIGINT);
             INSERT INTO sigiro_admin.alert_outbox (project_id, fingerprint, transition, payload_json, next_attempt_at_us)
               VALUES ('acme', 'f', 'firing', '{}', 1);",
        )
        .unwrap();
    })
    .await
    .unwrap();

    let mut conn = open_hosted_admin_db(&url).await.unwrap();
    let conn = tokio::task::spawn_blocking(move || {
        write_instance_status(&mut conn, "mode", "hosted").unwrap();
        assert_eq!(
            statuses(&mut conn),
            BTreeMap::from([
                ("mode".into(), "hosted".into()),
                ("version".into(), "0.1.0".into())
            ])
        );
        conn
    })
    .await
    .unwrap();
    drop(conn);

    tokio::task::spawn_blocking(move || {
        let mut check = sigiro::admin::connect_postgres(&url).unwrap();
        let next: i64 = check
            .query_one(
                "INSERT INTO sigiro_admin.alert_outbox (project_id, fingerprint, transition, payload_json, next_attempt_at_us, last_error)
                 VALUES ('acme', 'g', 'firing', '{}', 2, NULL) RETURNING id",
                &[],
            )
            .unwrap()
            .get(0);
        assert_eq!(next, 2, "the rebuilt outbox continues its id sequence after the copied row");
    })
    .await
    .unwrap();
}
