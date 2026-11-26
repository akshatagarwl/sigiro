use super::*;
use sigiro_segments::otlp::{InputFormat, transform_logs};

fn logs(project: &str, bodies: &[&str]) -> IngestMsg {
    let records: Vec<String> = bodies
        .iter()
        .map(|body| {
            format!(
                r#"{{"timeUnixNano":"1764000000000000000","severityText":"INFO","body":{{"stringValue":"{body}"}}}}"#
            )
        })
        .collect();
    let json = format!(
        r#"{{"resourceLogs":[{{"resource":{{"attributes":[{{"key":"service.name","value":{{"stringValue":"writer-test"}}}}]}},"scopeLogs":[{{"logRecords":[{}]}}]}}]}}"#,
        records.join(",")
    );
    let (batch, _) = transform_logs(json.as_bytes(), InputFormat::Json).expect("valid OTLP JSON");
    IngestMsg::Logs {
        project_id: project.to_string(),
        batch,
    }
}

/// A local DuckLake project opened exactly as `sigiro serve` opens it.
fn local_pool() -> (tempfile::TempDir, Arc<ProjectPool>) {
    let dir = tempfile::tempdir().unwrap();
    let data = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
    let pool = Arc::new(ProjectPool::new(
        data,
        8,
        crate::storage::StorageMode::LocalFile,
    ));
    (dir, pool)
}

fn schedule(idle: Duration) -> MaintenanceSchedule {
    MaintenanceSchedule {
        idle,
        ..MaintenanceSchedule::new(Duration::from_secs(3600), Some(256))
    }
}

fn router(pool: &Arc<ProjectPool>, idle: Duration) -> MsgRouter {
    MsgRouter::with_parts(
        Arc::clone(pool),
        DiskCapacityState::disabled(),
        None,
        QueueBudget::new(64 << 20, 16 << 20),
        schedule(idle),
    )
}

fn stored_logs(pool: &ProjectPool, project: &str) -> i64 {
    let readers = pool.create_project(project).unwrap();
    let reader = readers.get();
    reader
        .query_row("SELECT count(*) FROM lake.sigiro_logs", [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_is_acknowledged_once_its_rows_are_committed() {
    let (_dir, pool) = local_pool();
    let router = router(&pool, Duration::from_secs(60));
    router
        .send(logs("p", &["one", "two", "three"]))
        .await
        .unwrap();
    // No flush interval: the rows are queryable the moment send returns.
    assert_eq!(stored_logs(&pool, "p"), 3);
    router.shutdown_and_wait().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_requests_all_commit() {
    let (_dir, pool) = local_pool();
    let router = Arc::new(router(&pool, Duration::from_secs(60)));
    let sends: Vec<_> = (0..50)
        .map(|i| {
            let router = Arc::clone(&router);
            tokio::spawn(async move { router.send(logs("p", &[&format!("request {i}")])).await })
        })
        .collect();
    for send in sends {
        send.await.unwrap().unwrap();
    }
    assert_eq!(stored_logs(&pool, "p"), 50);
    router.shutdown_and_wait().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn small_commits_are_inlined_and_a_checkpoint_writes_one_file_per_table() {
    let (_dir, pool) = local_pool();
    let router = router(&pool, Duration::from_secs(60));
    for i in 0..40 {
        router
            .send(logs("p", &[&format!("turn {i}")]))
            .await
            .unwrap();
    }
    router.shutdown_and_wait().await.unwrap();

    let conn = pool.create_project_writer("p").unwrap();
    let (files, inlined) = crate::storage_health::catalog_counts(&conn).unwrap();
    // 40 log rows plus the storage-format row, all inlined: no Parquet yet.
    assert_eq!((files, inlined), (0, 41));
    assert!(matches!(run_checkpoint(&conn, 64), CheckpointOutcome::Done));
    let (files, inlined) = crate::storage_health::catalog_counts(&conn).unwrap();
    assert_eq!(
        (files, inlined),
        (2, 0),
        "one file each for sigiro_logs and storage_format"
    );
    pool.return_writer("p", conn);
    assert_eq!(stored_logs(&pool, "p"), 40);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_idle_writer_returns_its_connection_and_restarts_on_the_next_request() {
    let (_dir, pool) = local_pool();
    let router = router(&pool, Duration::from_millis(200));
    router.send(logs("p", &["before idle"])).await.unwrap();
    assert!(router.writers.contains_key("p"));

    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!router.writers.contains_key("p"), "idle writer exited");
    // The connection is back in the pool, so the project can be evicted.
    pool.return_writer("p", pool.take_writer("p").expect("writer returned"));

    router.send(logs("p", &["after idle"])).await.unwrap();
    assert_eq!(stored_logs(&pool, "p"), 2);
    router.shutdown_and_wait().await.unwrap();
}

#[test]
fn a_failed_commit_stores_nothing_and_the_next_one_succeeds() {
    let conn = crate::pool::open_duckdb().unwrap();
    conn.execute_batch("ATTACH ':memory:' AS lake").unwrap();
    let mut tables = Tables::new(
        conn,
        InliningLimits {
            rows: 0,
            profiles: 0,
        },
    );
    let good = logs("p", &["kept"]);
    // sigiro_logs does not exist yet: the whole group fails, atomically.
    assert!(tables.commit(&[&good, &good]).is_err());
    crate::schema::create_tables(&tables.conn, "lake.").unwrap();
    assert_eq!(tables.commit(&[&good, &good]).unwrap().rows, 2);
    let count: i64 = tables
        .conn
        .query_row("SELECT count(*) FROM lake.sigiro_logs", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn queued_bytes_are_bounded_and_a_full_budget_rejects_after_waiting() {
    let budget = QueueBudget::new(8 * PERMIT_BYTES, 4 * PERMIT_BYTES);
    let project = Arc::new(Semaphore::new(budget.project_permits as usize));
    let other = Arc::new(Semaphore::new(budget.project_permits as usize));

    assert_eq!(budget.permits_for(0), 1, "every request costs a permit");
    assert_eq!(
        budget.permits_for(100 * PERMIT_BYTES),
        4,
        "capped at the project share"
    );

    let wait = Duration::from_millis(50);
    let held = budget
        .acquire(&project, 4 * PERMIT_BYTES, wait)
        .await
        .unwrap();
    assert_eq!(budget.queued_bytes(), 4 * PERMIT_BYTES);
    assert!(
        budget.acquire(&project, 1, wait).await.is_none(),
        "the project's share is full"
    );
    let other_held = budget
        .acquire(&other, 4 * PERMIT_BYTES, wait)
        .await
        .unwrap();
    assert!(
        budget
            .acquire(&Arc::new(Semaphore::new(4)), 1, wait)
            .await
            .is_none(),
        "the global budget is full"
    );
    drop(held);
    drop(other_held);
    assert_eq!(budget.queued_bytes(), 0);
    assert!(budget.acquire(&project, 1, wait).await.is_some());
}

#[test]
fn checkpoint_retries_back_off_up_to_the_interval_and_never_stop() {
    let schedule = MaintenanceSchedule::new(Duration::from_secs(3600), Some(256));
    assert_eq!(schedule.file_budget, 64, "a quarter of 256 open files");
    let delays: Vec<u64> = (1..=10)
        .map(|failures| schedule.retry_after(failures).as_secs())
        .collect();
    assert_eq!(delays, [30, 60, 120, 240, 480, 960, 1920, 3600, 3600, 3600]);
    assert_eq!(schedule.retry_after(u32::MAX).as_secs(), 3600);
}

#[test]
fn many_new_files_bring_the_checkpoint_forward_unless_one_is_failing() {
    let mut maintenance = Maintenance::new(MaintenanceSchedule::new(
        Duration::from_secs(3600),
        Some(256),
    ));
    let now = Instant::now();
    assert!(!maintenance.is_due(now));
    maintenance.wrote(Written {
        rows: 64,
        files: 64,
    });
    assert!(maintenance.is_due(now));
    maintenance.failures = 1;
    assert!(
        !maintenance.is_due(now),
        "a failing checkpoint waits for its backoff"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_interrupts_a_writer_stuck_in_duckdb_within_one_deadline() {
    let conn = crate::pool::open_duckdb().unwrap();
    let (tx, _rx) = std_mpsc::channel();
    let handle = Arc::new(WriterHandle {
        id: 0,
        tx,
        open: parking_lot::Mutex::new(true),
        project_budget: Arc::new(Semaphore::new(1)),
        interrupt: conn.interrupt_handle(),
        thread: parking_lot::Mutex::new(None),
    });
    // A native statement that ignores the Stop command and runs for minutes.
    let thread = std::thread::spawn(move || {
        let result = conn.query_row("SELECT count(*) FROM range(1000000000000) a", [], |row| {
            row.get::<_, i64>(0)
        });
        assert!(result.is_err(), "the statement was interrupted");
    });
    *handle.thread.lock() = Some(thread);

    let started = Instant::now();
    let failures = stop_writers(vec![("p".to_string(), handle)], Duration::from_millis(200)).await;
    assert!(
        failures.is_empty(),
        "the interrupt stopped it: {failures:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_reports_a_writer_that_ignores_the_interrupt() {
    let conn = crate::pool::open_duckdb().unwrap();
    let (tx, _rx) = std_mpsc::channel();
    let handle = Arc::new(WriterHandle {
        id: 0,
        tx,
        open: parking_lot::Mutex::new(true),
        project_budget: Arc::new(Semaphore::new(1)),
        interrupt: conn.interrupt_handle(),
        thread: parking_lot::Mutex::new(None),
    });
    let thread = std::thread::spawn(|| std::thread::sleep(Duration::from_secs(5)));
    *handle.thread.lock() = Some(thread);

    let started = Instant::now();
    let failures = stop_writers(vec![("p".to_string(), handle)], Duration::from_millis(200)).await;
    assert_eq!(failures.len(), 1);
    assert!(failures[0].contains("did not stop"), "{failures:?}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "one deadline, not per-writer waits"
    );
}

#[test]
fn bounded_maintenance_merges_a_lake_with_too_many_files_in_batches() {
    let (_dir, pool) = local_pool();
    let conn = pool.create_project_writer("p").unwrap();
    conn.execute_batch("CALL lake.set_option('data_inlining_row_limit', 0)")
        .unwrap();
    for i in 0..120 {
        conn.execute_batch(&format!(
            "INSERT INTO lake.sigiro_logs (timestamp, observed_timestamp, service_name, \
             severity_number, severity_text, body) \
             VALUES (TIMESTAMP '2026-11-26 00:00:00', make_timestamp({i}), 'many-files', 9, 'INFO', 'row {i}')"
        ))
        .unwrap();
    }
    let before = crate::storage_health::data_file_count(&conn).unwrap();
    assert!(before >= 120, "one file per insert: {before}");

    // Batches of 16, as if the open-files limit were 64.
    bounded_maintenance(&conn, 16, Duration::from_secs(60)).unwrap();
    let after = crate::storage_health::data_file_count(&conn).unwrap();
    assert!(after <= 2, "{before} files merged down to {after}");
    let rows: i64 = conn
        .query_row(
            "SELECT count(*) FROM lake.sigiro_logs WHERE service_name = 'many-files'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 120);
}
