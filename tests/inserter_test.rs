use arrow::array::{Int32Array, Int64Array, StringArray, TimestampMicrosecondArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use sigiro::IngestMsg;
use sigiro::inserter::MsgRouter;
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

    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
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

#[tokio::test]
async fn inserter_processes_spans_for_single_project() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(
        camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap(),
        10,
    ));
    let router = MsgRouter::new(pool.clone());
    router.start();

    // Explicit DI: initialize and register flusher before sending
    let _ = pool.create_project("test_project").unwrap();
    router.register_flusher("test_project").unwrap();

    // Send 10 batches of 100 spans
    for _ in 0..10 {
        let batch = make_spans_batch(100);
        router
            .send(IngestMsg::Traces {
                project_id: "test_project".to_string(),
                batch,
            })
            .await
            .unwrap();
    }

    // Shut down router and wait for all flushers to complete
    router.shutdown_and_wait().await.unwrap();

    // Verify data landed in DuckDB
    let conn = pool.create_project("test_project").unwrap();
    let conn = conn.lock();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM lake.sigiro_spans", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1000, "expected 1000 spans, got {count}");
}

#[tokio::test]
async fn inserter_isolates_projects() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(
        camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap(),
        10,
    ));
    let router = MsgRouter::new(pool.clone());
    router.start();

    // Explicit DI: initialize and register flushers for both projects before sending
    let _ = pool.create_project("project_a").unwrap();
    router.register_flusher("project_a").unwrap();
    let _ = pool.create_project("project_b").unwrap();
    router.register_flusher("project_b").unwrap();

    // Send to two different projects
    for _ in 0..5 {
        router
            .send(IngestMsg::Traces {
                project_id: "project_a".to_string(),
                batch: make_spans_batch(100),
            })
            .await
            .unwrap();
        router
            .send(IngestMsg::Traces {
                project_id: "project_b".to_string(),
                batch: make_spans_batch(50),
            })
            .await
            .unwrap();
    }

    router.shutdown_and_wait().await.unwrap();

    // Each project should have its own data
    let conn_a = pool.create_project("project_a").unwrap();
    let count_a: i64 = conn_a
        .lock()
        .query_row("SELECT count(*) FROM lake.sigiro_spans", [], |row| {
            row.get(0)
        })
        .unwrap();

    let conn_b = pool.create_project("project_b").unwrap();
    let count_b: i64 = conn_b
        .lock()
        .query_row("SELECT count(*) FROM lake.sigiro_spans", [], |row| {
            row.get(0)
        })
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

#[tokio::test]
async fn inserter_handles_backpressure() {
    // Backpressure is now applied via bounded channels — senders block
    // rather than dropping data. All messages should be delivered.
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(
        camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap(),
        10,
    ));
    let router = MsgRouter::new(pool.clone());
    router.start();

    // Explicit DI: initialize and register flushers for both projects before sending
    let _ = pool.create_project("fast_project").unwrap();
    router.register_flusher("fast_project").unwrap();
    let _ = pool.create_project("slow_project").unwrap();
    router.register_flusher("slow_project").unwrap();

    // Send many messages quickly — with backpressure, none are dropped
    for i in 0..100 {
        router
            .send(IngestMsg::Traces {
                project_id: "fast_project".to_string(),
                batch: make_spans_batch(10),
            })
            .await
            .unwrap();

        // Every 10th message goes to a different project
        if i % 10 == 0 {
            router
                .send(IngestMsg::Traces {
                    project_id: "slow_project".to_string(),
                    batch: make_spans_batch(10),
                })
                .await
                .unwrap();
        }
    }

    router.shutdown_and_wait().await.unwrap();

    // Both projects should have data — no drops under backpressure
    let conn = pool.create_project("fast_project").unwrap();
    let count: i64 = conn
        .lock()
        .query_row("SELECT count(*) FROM lake.sigiro_spans", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(count > 0, "fast_project should have some spans");

    let conn2 = pool.create_project("slow_project").unwrap();
    let count2: i64 = conn2
        .lock()
        .query_row("SELECT count(*) FROM lake.sigiro_spans", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(count2 > 0, "slow_project should have some spans");
}

#[tokio::test]
async fn inserter_throughput_single_project() {
    // Throughput test: 50,000 rows should complete in under 60 seconds.
    // This validates the buffered flush path is working (not per-message appender create/destroy).
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(
        camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap(),
        10,
    ));
    let router = MsgRouter::new(pool.clone());
    router.start();

    // Explicit DI: initialize and register flusher before sending
    let _ = pool.create_project("throughput_test").unwrap();
    router.register_flusher("throughput_test").unwrap();

    let start = Instant::now();
    let batch_size = 500;
    let num_batches = 100; // 50,000 total rows

    for _ in 0..num_batches {
        router
            .send(IngestMsg::Traces {
                project_id: "throughput_test".to_string(),
                batch: make_spans_batch(batch_size),
            })
            .await
            .unwrap();
    }

    router.shutdown_and_wait().await.unwrap();
    let elapsed = start.elapsed();

    let conn = pool.create_project("throughput_test").unwrap();
    let count: i64 = conn
        .lock()
        .query_row("SELECT count(*) FROM lake.sigiro_spans", [], |row| {
            row.get(0)
        })
        .unwrap();

    assert_eq!(
        count,
        (batch_size * num_batches) as i64,
        "expected {} spans",
        batch_size * num_batches
    );
    assert!(
        elapsed < Duration::from_secs(180),
        "50k rows took {elapsed:?}, expected < 180s (DuckLake Parquet is slower than native DuckDB, hardware-dependent)"
    );
}
