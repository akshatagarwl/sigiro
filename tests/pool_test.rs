#![allow(clippy::disallowed_methods)]
// Test harnesses read and write the process environment directly to drive
// storage/config permutations. `clippy.toml` bans that in production code so
// every reader goes through typed-env; a test-only knob routed through
// typed-env would add a production config entry production never reads.

use sigiro::pool::TenantPool;
use std::sync::Once;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn test_pool(dir: &std::path::Path) -> TenantPool {
    test_pool_with_max(dir, 10)
}

fn test_extension_dir() -> String {
    std::env::var("SIGIRO_EXTENSION_DIR").unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(|home| format!("{home}/.duckdb/extensions"))
            .unwrap_or_else(|_| "/tmp/sigiro-test-extensions".to_string())
    })
}

fn test_pool_with_max(dir: &std::path::Path, max_connections: usize) -> TenantPool {
    static EXTENSION_ENV: Once = Once::new();
    EXTENSION_ENV.call_once(|| {
        // SAFETY: all tests in this binary use the same read-only extension cache.
        unsafe {
            std::env::set_var("SIGIRO_EXTENSION_DIR", test_extension_dir());
        }
    });
    TenantPool::new(
        camino::Utf8PathBuf::from_path_buf(dir.to_path_buf()).unwrap(),
        max_connections,
        sigiro::storage::StorageMode::LocalFile,
    )
}

#[test]
fn create_tenant_opens_connection() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());

    let readers = pool.create_tenant("proj_a").unwrap();
    assert_eq!(pool.connections.len(), 1);

    // Verify tables were created (now in lake.* schema)
    let conn = readers.get();
    let mut stmt = conn
        .prepare("SELECT count(*) FROM lake.sigiro_spans")
        .unwrap();
    let count: i64 = stmt.query_row([], |row| row.get(0)).unwrap();
    assert_eq!(count, 0);
}

#[test]
fn create_tenant_persists_safe_ducklake_policy() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());
    let readers = pool.create_tenant("proj_a").unwrap();
    let conn = readers.get();

    // Reproduce the unsafe policy persisted by older sigiro versions, then
    // reattach the existing catalog. Startup must migrate it to the safe value.
    conn.execute_batch(
        "CALL lake.set_option( \
             'data_inlining_row_limit', 1000, table_name => 'sigiro_profiles' \
         );",
    )
    .unwrap();
    let old_limit: String = conn
        .query_row(
            "SELECT value FROM lake.options() \
             WHERE option_name = 'data_inlining_row_limit' \
               AND scope_entry = 'main.sigiro_profiles'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_limit, "1000", "test must reproduce the old policy");

    drop(conn);
    drop(readers);
    pool.evict_idle(Duration::ZERO);
    let readers = pool.create_tenant("proj_a").unwrap();
    let conn = readers.get();

    let mut stmt = conn
        .prepare(
            "SELECT option_name, value, scope, scope_entry FROM lake.options() \
             WHERE option_name IN ( \
               'auto_compact', 'target_file_size', 'rewrite_delete_threshold', \
               'expire_older_than', 'delete_older_than', 'data_inlining_row_limit' \
             ) ORDER BY option_name",
        )
        .unwrap();
    let options: Vec<(String, String, String, Option<String>)> = stmt
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    assert_eq!(
        options,
        vec![
            ("auto_compact".into(), "true".into(), "GLOBAL".into(), None),
            (
                "data_inlining_row_limit".into(),
                "0".into(),
                "TABLE".into(),
                Some("main.sigiro_profiles".into()),
            ),
            (
                "delete_older_than".into(),
                "2 days".into(),
                "GLOBAL".into(),
                None
            ),
            (
                "expire_older_than".into(),
                "7 days".into(),
                "GLOBAL".into(),
                None
            ),
            (
                "rewrite_delete_threshold".into(),
                "0.200000".into(),
                "GLOBAL".into(),
                None,
            ),
            (
                "target_file_size".into(),
                "32000000".into(),
                "GLOBAL".into(),
                None
            ),
        ],
        "tenant creation must persist safe profile and CHECKPOINT policies"
    );
}

#[test]
fn get_returns_same_connection() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());

    let conn1 = pool.create_tenant("proj_a").unwrap();
    let conn2 = pool.create_tenant("proj_a").unwrap();
    assert!(conn1.same_pool(&conn2));
    assert_eq!(pool.connections.len(), 1);
}

#[test]
fn different_projects_different_connections() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());

    let conn_a = pool.create_tenant("proj_a").unwrap();
    let conn_b = pool.create_tenant("proj_b").unwrap();
    assert!(!conn_a.same_pool(&conn_b));
    assert_eq!(pool.connections.len(), 2);
}

#[test]
fn data_isolation_between_projects() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());

    // Insert into tenant A using raw SQL with all required columns
    let conn_a = pool.create_tenant("proj_a").unwrap();
    {
        let conn = conn_a.get();
        conn.execute_batch(
            "INSERT INTO lake.sigiro_metrics_gauge (timestamp, metric_name, service_name, value) \
                 VALUES (current_timestamp, 'cpu', 'svc', 42.0)",
        )
        .unwrap();
    }

    // Tenant B should have no data
    let conn_b = pool.create_tenant("proj_b").unwrap();
    {
        let conn = conn_b.get();
        let mut stmt = conn
            .prepare("SELECT count(*) FROM lake.sigiro_metrics_gauge")
            .unwrap();
        let count: i64 = stmt.query_row([], |row| row.get(0)).unwrap();
        assert_eq!(count, 0, "tenant B should have no data from tenant A");
    }
}

#[test]
fn reader_queries_do_not_wait_for_writer_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());

    let readers = pool.create_tenant("proj_a").unwrap();
    let writer = pool.take_writer("proj_a").unwrap();
    let (writer_started_tx, writer_started_rx) = mpsc::channel();
    let (commit_tx, commit_rx) = mpsc::channel();

    let writer_thread = std::thread::spawn(move || {
        writer
            .execute_batch(
                "BEGIN TRANSACTION;
                 INSERT INTO lake.sigiro_metrics_gauge (timestamp, metric_name, service_name, value)
                 VALUES (current_timestamp, 'cpu', 'svc', 42.0);",
            )
            .unwrap();
        writer_started_tx.send(()).unwrap();
        commit_rx.recv().unwrap();
        writer.execute_batch("COMMIT;").unwrap();
        writer
    });

    writer_started_rx.recv().unwrap();
    let start = Instant::now();
    let reader = readers.get();
    let count_before_commit: i64 = reader
        .query_row(
            "SELECT count(*) FROM lake.sigiro_metrics_gauge",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count_before_commit, 0);
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "reader query waited for writer transaction"
    );
    drop(reader);

    commit_tx.send(()).unwrap();
    let writer = writer_thread.join().unwrap();
    pool.return_writer("proj_a", writer);

    let reader = readers.get();
    let count_after_commit: i64 = reader
        .query_row(
            "SELECT count(*) FROM lake.sigiro_metrics_gauge",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count_after_commit, 1);
}

#[test]
fn evict_idle_removes_old_connections() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());

    pool.create_tenant("proj_a").unwrap();
    pool.create_tenant("proj_b").unwrap();
    assert_eq!(pool.connections.len(), 2);

    // Evict with zero duration — everything is "idle"
    pool.evict_idle(std::time::Duration::ZERO);
    assert_eq!(pool.connections.len(), 0);
}

#[test]
fn evict_oldest_at_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool_with_max(dir.path(), 2);

    pool.create_tenant("proj_a").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    pool.create_tenant("proj_b").unwrap();
    assert_eq!(pool.connections.len(), 2);

    // Opening a third should evict the oldest (proj_a)
    pool.create_tenant("proj_c").unwrap();
    assert_eq!(pool.connections.len(), 2);
    assert!(pool.connections.contains_key("proj_b"));
    assert!(pool.connections.contains_key("proj_c"));
}

#[test]
fn local_mode_locks_filesystem_access() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());
    let readers = pool.create_tenant("proj_a").unwrap();
    let conn = readers.get();
    for call in [
        "read_csv('/etc/passwd')",
        "read_text('/etc/passwd')",
        "read_blob('/etc/passwd')",
        "read_json('/etc/passwd')",
        "glob('/etc/*')",
    ] {
        let sql = format!("SELECT count(*) FROM {call}");
        let res = conn
            .prepare(&sql)
            .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)));
        let msg = format!("{:?}", res.err());
        assert!(
            msg.contains("disabled by configuration"),
            "{call} should be engine-blocked, got: {msg}"
        );
    }
    let mut stmt = conn
        .prepare("SELECT count(*) FROM lake.sigiro_spans")
        .unwrap();
    assert_eq!(stmt.query_row([], |r| r.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn remove_project() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());

    pool.create_tenant("proj_a").unwrap();
    assert_eq!(pool.connections.len(), 1);

    pool.remove("proj_a");
    assert_eq!(pool.connections.len(), 0);
}

/// Retention must sweep every tenant that has data, not only the ones that
/// happen to hold a pooled connection at the moment it runs.
///
/// `MsgRouter::run_retention_cleanup` iterates the pool's live connection map.
/// `run_evictor` drops any tenant idle for `MAX_IDLE_SECS` (300) while
/// retention fires hourly, so a low-traffic tenant is almost never in the map
/// when the sweep happens and is silently skipped forever.
///
/// Measured on the hosted box on 2026-08-31: one tenant carried 3,227 snapshots
/// and 3,030 data files spanning nine days under a seven-day policy, and the
/// container log showed a single retention run, for the one tenant that ingests
/// continuously and is therefore always pooled.
#[test]
fn known_tenant_ids_survives_eviction() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());

    pool.create_tenant("proj_a").unwrap();
    pool.create_tenant("proj_b").unwrap();

    let while_pooled = pool.known_tenant_ids();
    assert!(
        while_pooled.iter().any(|id| id == "proj_a")
            && while_pooled.iter().any(|id| id == "proj_b"),
        "both projects must be known while pooled, got {while_pooled:?}"
    );

    // Exactly what the idle evictor does to a tenant with no recent traffic.
    pool.evict_idle(std::time::Duration::ZERO);
    assert_eq!(pool.connections.len(), 0, "eviction emptied the pool");

    let after_eviction = pool.known_tenant_ids();
    assert!(
        after_eviction.iter().any(|id| id == "proj_a")
            && after_eviction.iter().any(|id| id == "proj_b"),
        "a tenant that has been evicted still has data on disk and still needs \
         retention, but the retention sweep would not see it: {after_eviction:?}"
    );
}

/// The enumeration must not hand retention the shared directories that sit
/// beside the per-tenant ones. `extensions` and `tenants` are reserved by
/// `storage::sanitize_tenant_id` for exactly this reason.
#[test]
fn known_tenant_ids_excludes_reserved_directories() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());
    pool.create_tenant("proj_a").unwrap();
    std::fs::create_dir_all(dir.path().join("extensions")).unwrap();
    std::fs::create_dir_all(dir.path().join("tenants")).unwrap();

    let ids = pool.known_tenant_ids();
    assert!(
        ids.iter().any(|id| id == "proj_a"),
        "real tenant missing: {ids:?}"
    );
    assert!(
        !ids.iter().any(|id| id == "extensions" || id == "tenants"),
        "reserved directories must not be swept as tenants: {ids:?}"
    );
}
