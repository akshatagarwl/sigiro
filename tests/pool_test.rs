use sigiro::pool::ProjectPool;
use std::sync::Arc;

fn test_pool(dir: &std::path::Path) -> ProjectPool {
    ProjectPool::new(dir.to_path_buf(), "", 10)
}

#[test]
fn get_or_create_opens_connection() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());

    let conn = pool.get_or_create("proj_a").unwrap();
    assert_eq!(pool.len(), 1);

    // Verify tables were created
    let guard = conn.lock().unwrap();
    let mut stmt = guard.prepare("SELECT count(*) FROM sigiro_spans").unwrap();
    let count: i64 = stmt.query_row([], |row| row.get(0)).unwrap();
    assert_eq!(count, 0);
}

#[test]
fn get_returns_same_connection() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());

    let conn1 = pool.get_or_create("proj_a").unwrap();
    let conn2 = pool.get_or_create("proj_a").unwrap();
    assert!(Arc::ptr_eq(&conn1, &conn2));
    assert_eq!(pool.len(), 1);
}

#[test]
fn different_projects_different_connections() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());

    let conn_a = pool.get_or_create("proj_a").unwrap();
    let conn_b = pool.get_or_create("proj_b").unwrap();
    assert!(!Arc::ptr_eq(&conn_a, &conn_b));
    assert_eq!(pool.len(), 2);
}

#[test]
fn data_isolation_between_projects() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());

    // Insert into project A using raw SQL with all required columns
    let conn_a = pool.get_or_create("proj_a").unwrap();
    {
        let guard = conn_a.lock().unwrap();
        guard
            .execute_batch(
                "INSERT INTO sigiro_metrics_gauge (timestamp, metric_name, service_name, value) \
                 VALUES (current_timestamp, 'cpu', 'svc', 42.0)",
            )
            .unwrap();
    }

    // Project B should have no data
    let conn_b = pool.get_or_create("proj_b").unwrap();
    {
        let guard = conn_b.lock().unwrap();
        let mut stmt = guard
            .prepare("SELECT count(*) FROM sigiro_metrics_gauge")
            .unwrap();
        let count: i64 = stmt.query_row([], |row| row.get(0)).unwrap();
        assert_eq!(count, 0, "project B should have no data from project A");
    }
}

#[test]
fn evict_idle_removes_old_connections() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());

    pool.get_or_create("proj_a").unwrap();
    pool.get_or_create("proj_b").unwrap();
    assert_eq!(pool.len(), 2);

    // Evict with zero duration — everything is "idle"
    pool.evict_idle(std::time::Duration::ZERO);
    assert_eq!(pool.len(), 0);
}

#[test]
fn evict_oldest_at_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let pool = ProjectPool::new(dir.path().to_path_buf(), "", 2);

    pool.get_or_create("proj_a").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    pool.get_or_create("proj_b").unwrap();
    assert_eq!(pool.len(), 2);

    // Opening a third should evict the oldest (proj_a)
    pool.get_or_create("proj_c").unwrap();
    assert_eq!(pool.len(), 2);
    assert!(pool.connections.contains_key("proj_b"));
    assert!(pool.connections.contains_key("proj_c"));
}

#[test]
fn remove_project() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());

    pool.get_or_create("proj_a").unwrap();
    assert_eq!(pool.len(), 1);

    pool.remove("proj_a");
    assert_eq!(pool.len(), 0);
}
