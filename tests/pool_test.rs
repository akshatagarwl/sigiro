use sigiro::pool::ProjectPool;
use std::sync::Once;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn test_pool(dir: &std::path::Path) -> ProjectPool {
    test_pool_with_max(dir, 10)
}

fn test_extension_dir() -> String {
    std::env::var("SIGIRO_EXTENSION_DIR").unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(|home| format!("{home}/.duckdb/extensions"))
            .unwrap_or_else(|_| "/tmp/sigiro-test-extensions".to_string())
    })
}

fn test_pool_with_max(dir: &std::path::Path, max_connections: usize) -> ProjectPool {
    static EXTENSION_ENV: Once = Once::new();
    EXTENSION_ENV.call_once(|| {
        // SAFETY: all tests in this binary use the same read-only extension cache.
        unsafe {
            std::env::set_var("SIGIRO_EXTENSION_DIR", test_extension_dir());
        }
    });
    ProjectPool::new(
        camino::Utf8PathBuf::from_path_buf(dir.to_path_buf()).unwrap(),
        max_connections,
        sigiro::storage::StorageMode::LocalFile,
    )
}

#[test]
fn create_project_opens_connection() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());

    let readers = pool.create_project("proj_a").unwrap();
    assert_eq!(pool.len(), 1);

    // Verify tables were created (now in lake.* schema)
    let conn = readers.get();
    let mut stmt = conn
        .prepare("SELECT count(*) FROM lake.sigiro_spans")
        .unwrap();
    let count: i64 = stmt.query_row([], |row| row.get(0)).unwrap();
    assert_eq!(count, 0);
}

#[test]
fn get_returns_same_connection() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());

    let conn1 = pool.create_project("proj_a").unwrap();
    let conn2 = pool.create_project("proj_a").unwrap();
    assert!(conn1.same_pool(&conn2));
    assert_eq!(pool.len(), 1);
}

#[test]
fn different_projects_different_connections() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());

    let conn_a = pool.create_project("proj_a").unwrap();
    let conn_b = pool.create_project("proj_b").unwrap();
    assert!(!conn_a.same_pool(&conn_b));
    assert_eq!(pool.len(), 2);
}

#[test]
fn data_isolation_between_projects() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());

    // Insert into project A using raw SQL with all required columns
    let conn_a = pool.create_project("proj_a").unwrap();
    {
        let conn = conn_a.get();
        conn.execute_batch(
            "INSERT INTO lake.sigiro_metrics_gauge (timestamp, metric_name, service_name, value) \
                 VALUES (current_timestamp, 'cpu', 'svc', 42.0)",
        )
        .unwrap();
    }

    // Project B should have no data
    let conn_b = pool.create_project("proj_b").unwrap();
    {
        let conn = conn_b.get();
        let mut stmt = conn
            .prepare("SELECT count(*) FROM lake.sigiro_metrics_gauge")
            .unwrap();
        let count: i64 = stmt.query_row([], |row| row.get(0)).unwrap();
        assert_eq!(count, 0, "project B should have no data from project A");
    }
}

#[test]
fn reader_queries_do_not_wait_for_writer_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());

    let readers = pool.create_project("proj_a").unwrap();
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

    pool.create_project("proj_a").unwrap();
    pool.create_project("proj_b").unwrap();
    assert_eq!(pool.len(), 2);

    // Evict with zero duration — everything is "idle"
    pool.evict_idle(std::time::Duration::ZERO);
    assert_eq!(pool.len(), 0);
}

#[test]
fn evict_oldest_at_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool_with_max(dir.path(), 2);

    pool.create_project("proj_a").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    pool.create_project("proj_b").unwrap();
    assert_eq!(pool.len(), 2);

    // Opening a third should evict the oldest (proj_a)
    pool.create_project("proj_c").unwrap();
    assert_eq!(pool.len(), 2);
    assert!(pool.connections.contains_key("proj_b"));
    assert!(pool.connections.contains_key("proj_c"));
}

#[test]
fn local_mode_locks_filesystem_access() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());
    let readers = pool.create_project("proj_a").unwrap();
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

    pool.create_project("proj_a").unwrap();
    assert_eq!(pool.len(), 1);

    pool.remove("proj_a");
    assert_eq!(pool.len(), 0);
}
