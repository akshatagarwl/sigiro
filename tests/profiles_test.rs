//! Tests for OTLP Profiles: dictionary resolution, inserter pipeline, investigate queries.

use arrow::array::{Int64Array, ListArray, StringArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use sigiro::IngestMsg;
use sigiro::inserter::MsgRouter;
use sigiro::investigate::QueryCtx;
use sigiro::pool::ProjectPool;
use std::sync::Arc;

/// Build a profiles RecordBatch manually (simulating what transform_profiles produces).
fn make_profiles_batch(
    service_name: &str,
    ts_base: i64,
    stacks: &[(&[&str], i64, &str, &str)], // (stack_frames, value, profile_type, unit)
) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![
        Field::new("profile_id", DataType::Utf8, true),
        Field::new("service_name", DataType::Utf8, false),
        Field::new(
            "timestamp",
            DataType::Timestamp(TimeUnit::Microsecond, None),
            false,
        ),
        Field::new("profile_type", DataType::Utf8, true),
        Field::new(
            "stack_trace",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            true,
        ),
        Field::new("value", DataType::Int64, true),
        Field::new("unit", DataType::Utf8, true),
        Field::new("resource_attributes", DataType::Utf8, true),
    ]));

    let mut profile_ids = Vec::new();
    let mut service_names = Vec::new();
    let mut timestamps = Vec::new();
    let mut profile_types = Vec::new();
    let mut values = Vec::new();
    let mut units = Vec::new();
    let mut resource_attrs: Vec<Option<&str>> = Vec::new();

    // Build list array for stack traces
    let mut stack_offsets: Vec<i32> = vec![0];
    let mut stack_values_flat: Vec<&str> = Vec::new();

    for (i, (frames, value, ptype, unit)) in stacks.iter().enumerate() {
        profile_ids.push("test-profile-001");
        service_names.push(service_name);
        timestamps.push(ts_base + i as i64);
        profile_types.push(*ptype);
        values.push(*value);
        units.push(*unit);
        resource_attrs.push(None);

        for frame in *frames {
            stack_values_flat.push(frame);
        }
        let prev = *stack_offsets.last().unwrap();
        stack_offsets.push(prev + frames.len() as i32);
    }

    let stack_list = ListArray::try_new(
        Arc::new(Field::new("item", DataType::Utf8, true)),
        arrow::buffer::OffsetBuffer::new(stack_offsets.into()),
        Arc::new(StringArray::from(stack_values_flat)),
        None,
    )
    .unwrap();

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(profile_ids)),
            Arc::new(StringArray::from(service_names)),
            Arc::new(arrow::array::TimestampMicrosecondArray::from(timestamps)),
            Arc::new(StringArray::from(profile_types)),
            Arc::new(stack_list),
            Arc::new(Int64Array::from(values)),
            Arc::new(StringArray::from(units)),
            Arc::new(StringArray::from(resource_attrs)),
        ],
    )
    .unwrap()
}

// ---------------------------------------------------------------------------
// E2E: profiles through inserter → DuckDB → investigate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_profiles_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(dir.path().to_path_buf(), 10));
    let router = MsgRouter::new(pool.clone());

    let now_us = chrono::Utc::now().timestamp_micros();

    // Send CPU profile samples with known stack traces
    let stacks = vec![
        (
            &["main", "handle_request", "db_query"][..],
            15_000_000i64,
            "cpu",
            "nanoseconds",
        ),
        (
            &["main", "handle_request", "serialize"],
            8_000_000,
            "cpu",
            "nanoseconds",
        ),
        (
            &["main", "handle_request", "db_query"],
            12_000_000,
            "cpu",
            "nanoseconds",
        ),
        (&["main", "gc_collect"], 3_000_000, "cpu", "nanoseconds"),
    ];

    let _ = router
        .send(IngestMsg::Profiles {
            project_id: "profile_test".to_string(),
            batch: make_profiles_batch("api-svc", now_us, &stacks),
        })
        .await;

    router.shutdown_and_wait().await;

    // Verify rows landed in DuckDB
    let conn = pool.get_or_create("profile_test").unwrap();
    let conn = conn.lock().unwrap();

    let total: i64 = conn
        .query_row("SELECT count(*) FROM lake.sigiro_profiles", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(total, 4, "should have 4 profile samples");

    // Verify stack traces are stored correctly
    let db_query_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM lake.sigiro_profiles WHERE list_contains(stack_trace, 'db_query')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(db_query_count, 2, "should have 2 samples with db_query");

    // Verify profile_type stored
    let cpu_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM lake.sigiro_profiles WHERE profile_type = 'cpu'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cpu_count, 4, "all 4 samples should be cpu type");

    // Verify via investigate query: top functions
    let qctx = QueryCtx {
        conn: &conn,
        service: "api-svc",
        from_ts: now_us - 60_000_000,
        to_ts: now_us + 60_000_000,
        prefix: "lake.",
    };
    let profiles = sigiro::investigate::query_profiles(&qctx, 20).unwrap();
    assert!(!profiles.is_empty(), "should have profile summaries");

    // db_query should have highest total value (15M + 12M = 27M)
    let db_query_summary = profiles.iter().find(|p| p.function == "db_query");
    assert!(
        db_query_summary.is_some(),
        "db_query should appear in profile summaries"
    );
    assert_eq!(
        db_query_summary.unwrap().total_value,
        27_000_000,
        "db_query total should be 27M ns"
    );
}

// ---------------------------------------------------------------------------
// Multi-type profiles (CPU + heap in same project)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_profiles_multi_type() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(dir.path().to_path_buf(), 10));
    let router = MsgRouter::new(pool.clone());

    let now_us = chrono::Utc::now().timestamp_micros();

    // CPU samples
    let cpu_stacks = vec![(
        &["main", "compute"][..],
        50_000_000i64,
        "cpu",
        "nanoseconds",
    )];
    // Heap samples
    let heap_stacks = vec![(
        &["main", "allocate_buffer"][..],
        1_048_576i64,
        "alloc_space",
        "bytes",
    )];

    let _ = router
        .send(IngestMsg::Profiles {
            project_id: "multi_type".to_string(),
            batch: make_profiles_batch("api-svc", now_us, &cpu_stacks),
        })
        .await;
    let _ = router
        .send(IngestMsg::Profiles {
            project_id: "multi_type".to_string(),
            batch: make_profiles_batch("api-svc", now_us + 100, &heap_stacks),
        })
        .await;

    router.shutdown_and_wait().await;

    let conn = pool.get_or_create("multi_type").unwrap();
    let conn = conn.lock().unwrap();

    // Should have both profile types
    let type_count: i64 = conn
        .query_row(
            "SELECT count(DISTINCT profile_type) FROM lake.sigiro_profiles",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(type_count, 2, "should have 2 distinct profile types");

    // Query profiles — should see both types
    let qctx = QueryCtx {
        conn: &conn,
        service: "api-svc",
        from_ts: now_us - 60_000_000,
        to_ts: now_us + 60_000_000,
        prefix: "lake.",
    };
    let profiles = sigiro::investigate::query_profiles(&qctx, 20).unwrap();
    assert_eq!(
        profiles.len(),
        2,
        "should have 2 profile summaries (cpu + heap)"
    );

    let cpu = profiles.iter().find(|p| p.profile_type == "cpu");
    let heap = profiles.iter().find(|p| p.profile_type == "alloc_space");
    assert!(cpu.is_some(), "should have cpu profile");
    assert!(heap.is_some(), "should have alloc_space profile");
}

// ---------------------------------------------------------------------------
// Empty profiles batch doesn't crash
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_profiles_empty_batch() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(dir.path().to_path_buf(), 10));
    let router = MsgRouter::new(pool.clone());

    let now_us = chrono::Utc::now().timestamp_micros();

    // Empty profiles batch
    let _ = router
        .send(IngestMsg::Profiles {
            project_id: "empty_profiles".to_string(),
            batch: make_profiles_batch("api-svc", now_us, &[]),
        })
        .await;

    router.shutdown_and_wait().await;

    let conn = pool.get_or_create("empty_profiles").unwrap();
    let conn = conn.lock().unwrap();

    let total: i64 = conn
        .query_row("SELECT count(*) FROM lake.sigiro_profiles", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(total, 0, "should have 0 profile samples");
}
