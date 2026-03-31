use arrow::array::{Int32Array, Int64Array, StringArray, TimestampMicrosecondArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use crossbeam_channel::unbounded;
use sigiro::ingest::IngestMsg;
use sigiro::pool::ProjectPool;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Create a minimal spans RecordBatch matching sigiro_spans schema.
fn make_spans_batch(n: usize) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![
        Field::new(
            "timestamp",
            DataType::Timestamp(TimeUnit::Microsecond, None),
            false,
        ),
        Field::new("end_timestamp", DataType::Int64, false),
        Field::new("duration", DataType::Int64, false),
        Field::new("trace_id", DataType::Utf8, true),
        Field::new("span_id", DataType::Utf8, true),
        Field::new("parent_span_id", DataType::Utf8, true),
        Field::new("trace_state", DataType::Utf8, true),
        Field::new("service_name", DataType::Utf8, false),
        Field::new("service_namespace", DataType::Utf8, true),
        Field::new("service_instance_id", DataType::Utf8, true),
        Field::new("span_name", DataType::Utf8, false),
        Field::new("span_kind", DataType::Int32, false),
        Field::new("status_code", DataType::Int32, false),
        Field::new("status_message", DataType::Utf8, true),
        Field::new("resource_attributes", DataType::Utf8, true),
        Field::new("scope_name", DataType::Utf8, true),
        Field::new("scope_version", DataType::Utf8, true),
        Field::new("scope_attributes", DataType::Utf8, true),
        Field::new("span_attributes", DataType::Utf8, true),
        Field::new("events_json", DataType::Utf8, true),
        Field::new("links_json", DataType::Utf8, true),
        Field::new("dropped_attributes_count", DataType::Int32, true),
        Field::new("dropped_events_count", DataType::Int32, true),
        Field::new("dropped_links_count", DataType::Int32, true),
        Field::new("flags", DataType::Int32, true),
    ]));

    let now_us = chrono::Utc::now().timestamp_micros();
    let timestamps: Vec<i64> = (0..n).map(|i| now_us + i as i64).collect();
    let end_timestamps: Vec<i64> = timestamps.iter().map(|t| t + 1000).collect();
    let durations: Vec<i64> = vec![1000i64; n];
    let strings: Vec<&str> = vec!["test"; n];
    let nulls: Vec<Option<&str>> = vec![None; n];
    let zeros: Vec<i32> = vec![0i32; n];
    let null_ints: Vec<Option<i32>> = vec![None; n];

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(TimestampMicrosecondArray::from(timestamps)),
            Arc::new(Int64Array::from(end_timestamps)),
            Arc::new(Int64Array::from(durations)),
            Arc::new(StringArray::from(strings.clone())), // trace_id
            Arc::new(StringArray::from(strings.clone())), // span_id
            Arc::new(StringArray::from(nulls.clone())),   // parent_span_id
            Arc::new(StringArray::from(nulls.clone())),   // trace_state
            Arc::new(StringArray::from(strings.clone())), // service_name
            Arc::new(StringArray::from(nulls.clone())),   // service_namespace
            Arc::new(StringArray::from(nulls.clone())),   // service_instance_id
            Arc::new(StringArray::from(strings.clone())), // span_name
            Arc::new(Int32Array::from(zeros.clone())),    // span_kind
            Arc::new(Int32Array::from(zeros.clone())),    // status_code
            Arc::new(StringArray::from(nulls.clone())),   // status_message
            Arc::new(StringArray::from(nulls.clone())),   // resource_attributes
            Arc::new(StringArray::from(nulls.clone())),   // scope_name
            Arc::new(StringArray::from(nulls.clone())),   // scope_version
            Arc::new(StringArray::from(nulls.clone())),   // scope_attributes
            Arc::new(StringArray::from(nulls.clone())),   // span_attributes
            Arc::new(StringArray::from(nulls.clone())),   // events_json
            Arc::new(StringArray::from(nulls.clone())),   // links_json
            Arc::new(Int32Array::from(null_ints.clone())), // dropped_attributes_count
            Arc::new(Int32Array::from(null_ints.clone())), // dropped_events_count
            Arc::new(Int32Array::from(null_ints.clone())), // dropped_links_count
            Arc::new(Int32Array::from(null_ints)),        // flags
        ],
    )
    .expect("valid record batch")
}

#[test]
fn inserter_processes_spans_for_single_project() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(dir.path().to_path_buf(), "", 10));
    let (tx, rx) = unbounded();

    let pool_clone = Arc::clone(&pool);
    let wal_dir = dir.path().join("wal");
    let handle = std::thread::spawn(move || {
        sigiro::inserter::run_inserter(&pool_clone, rx, wal_dir).unwrap();
    });

    // Send 10 batches of 100 spans
    for _ in 0..10 {
        let batch = make_spans_batch(100);
        tx.send(IngestMsg::Traces {
            project_id: "test_project".to_string(),
            batch,
        })
        .unwrap();
    }

    // Drop sender to close channel and let inserter finish
    drop(tx);
    handle.join().unwrap();

    // Verify data landed in DuckDB
    let conn = pool.get_or_create("test_project").unwrap();
    let conn = conn.lock().unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM sigiro_spans", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1000, "expected 1000 spans, got {count}");
}

#[test]
fn inserter_isolates_projects() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(dir.path().to_path_buf(), "", 10));
    let (tx, rx) = unbounded();

    let pool_clone = Arc::clone(&pool);
    let wal_dir = dir.path().join("wal");
    let handle = std::thread::spawn(move || {
        sigiro::inserter::run_inserter(&pool_clone, rx, wal_dir).unwrap();
    });

    // Send to two different projects
    for _ in 0..5 {
        tx.send(IngestMsg::Traces {
            project_id: "project_a".to_string(),
            batch: make_spans_batch(100),
        })
        .unwrap();
        tx.send(IngestMsg::Traces {
            project_id: "project_b".to_string(),
            batch: make_spans_batch(50),
        })
        .unwrap();
    }

    drop(tx);
    handle.join().unwrap();

    // Each project should have its own data
    let conn_a = pool.get_or_create("project_a").unwrap();
    let count_a: i64 = conn_a
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM sigiro_spans", [], |row| row.get(0))
        .unwrap();

    let conn_b = pool.get_or_create("project_b").unwrap();
    let count_b: i64 = conn_b
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM sigiro_spans", [], |row| row.get(0))
        .unwrap();

    assert_eq!(
        count_a, 500,
        "project_a should have 500 spans, got {count_a}"
    );
    assert_eq!(
        count_b, 250,
        "project_b should have 250 spans, got {count_b}"
    );
}

#[test]
fn inserter_handles_bounded_channel_backpressure() {
    // This tests that per-project bounded channels handle backpressure gracefully.
    // The inserter should drop messages for a slow project rather than blocking others.
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(dir.path().to_path_buf(), "", 10));
    let (tx, rx) = unbounded(); // dispatcher channel is unbounded

    let pool_clone = Arc::clone(&pool);
    let wal_dir = dir.path().join("wal");
    let handle = std::thread::spawn(move || {
        sigiro::inserter::run_inserter(&pool_clone, rx, wal_dir).unwrap();
    });

    // Send many messages quickly — some may be dropped due to bounded per-project channel
    for i in 0..100 {
        let _ = tx.send(IngestMsg::Traces {
            project_id: "fast_project".to_string(),
            batch: make_spans_batch(10),
        });
        // Every 10th message goes to a different project
        if i % 10 == 0 {
            let _ = tx.send(IngestMsg::Traces {
                project_id: "slow_project".to_string(),
                batch: make_spans_batch(10),
            });
        }
    }

    drop(tx);
    handle.join().unwrap();

    // Both projects should have data (we don't assert exact counts because
    // some messages may be dropped under backpressure, which is correct behavior)
    let conn = pool.get_or_create("fast_project").unwrap();
    let count: i64 = conn
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM sigiro_spans", [], |row| row.get(0))
        .unwrap();
    assert!(count > 0, "fast_project should have some spans");
}

#[test]
fn inserter_throughput_single_project() {
    // Throughput test: 50,000 rows should complete in under 5 seconds.
    // This validates the buffered flush path is working (not per-message appender create/destroy).
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(dir.path().to_path_buf(), "", 10));
    let (tx, rx) = unbounded();

    let pool_clone = Arc::clone(&pool);
    let wal_dir = dir.path().join("wal");
    let handle = std::thread::spawn(move || {
        sigiro::inserter::run_inserter(&pool_clone, rx, wal_dir).unwrap();
    });

    let start = Instant::now();
    let batch_size = 500;
    let num_batches = 100; // 50,000 total rows

    for _ in 0..num_batches {
        tx.send(IngestMsg::Traces {
            project_id: "throughput_test".to_string(),
            batch: make_spans_batch(batch_size),
        })
        .unwrap();
    }

    drop(tx);
    handle.join().unwrap();
    let elapsed = start.elapsed();

    let conn = pool.get_or_create("throughput_test").unwrap();
    let count: i64 = conn
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM sigiro_spans", [], |row| row.get(0))
        .unwrap();

    assert_eq!(
        count,
        (batch_size * num_batches) as i64,
        "expected {} spans",
        batch_size * num_batches
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "50k rows took {elapsed:?}, expected < 5s"
    );
}
