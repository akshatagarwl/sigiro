//! Black-box migration contract tests. They exercise the public migration API
//! through the compiled test crate and prove the unsafe partial path refuses
//! before it can contact PostgreSQL or mutate the source.

use rusqlite::Connection;
use tempfile::tempdir;

#[test]
fn local_ducklake_metadata_fails_closed_before_target_attach() {
    let dir = tempdir().unwrap();
    let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
    let admin = Connection::open(root.join("admin.sqlite")).unwrap();
    admin
        .execute_batch(
            "CREATE TABLE instance_status (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO instance_status VALUES ('version', 'test');",
        )
        .unwrap();
    std::fs::create_dir_all(root.join("default")).unwrap();
    std::fs::write(root.join("default/catalog.ducklake"), b"DuckDB").unwrap();

    let error = sigiro::migration::migrate_admin_sqlite(
        &root,
        "postgresql://sigiro:password@127.0.0.1:1/ducklake",
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("refusing partial migration"));
    assert!(error.contains("no safe metadata export/import API"));
    assert!(root.join("admin.sqlite").exists());
}

#[test]
fn preflight_identifies_ducklake_file_catalog() {
    let dir = tempdir().unwrap();
    let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
    assert!(!sigiro::migration::has_unsupported_ducklake_metadata(&root));
    std::fs::create_dir_all(root.join("default")).unwrap();
    std::fs::write(root.join("default/catalog.ducklake"), b"DuckDB").unwrap();
    assert!(sigiro::migration::has_unsupported_ducklake_metadata(&root));
}

#[test]
fn hosted_migration_requires_verified_tls_before_connecting() {
    let dir = tempdir().unwrap();
    let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
    let admin = Connection::open(root.join("admin.sqlite")).unwrap();
    admin
        .execute_batch("CREATE TABLE instance_status (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
        .unwrap();

    let error = sigiro::migration::migrate_admin_sqlite(
        &root,
        "postgresql://sigiro:secret@127.0.0.1:1/sigiro",
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("sslmode=verify-full"));

    let error = sigiro::migration::migrate_admin_sqlite(
        &root,
        "postgresql://sigiro:secret@127.0.0.1:1/sigiro?sslmode=verify-full",
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("sslrootcert"));
    assert!(root.join("admin.sqlite").exists());
}
