//! End-to-end tests for the sigiro observability pipeline.
//!
//! This test verifies the full OTLP ingest → DuckDB → diagnose path.

#![allow(clippy::disallowed_methods)]
// Test harnesses read and write the process environment directly to drive
// storage/config permutations. `clippy.toml` bans that in production code so
// every reader goes through typed-env; a test-only knob routed through
// typed-env would add a production config entry production never reads.

use arrow::array::{Int32Array, Int64Array, StringArray, TimestampMicrosecondArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use sigiro::IngestMsg;
use sigiro::diagnose::{DiagnoseRequest, DiagnoseServices, diagnose};
use sigiro::inserter::MsgRouter;
use sigiro::pool::ProjectPool;
use std::sync::{Arc, Once};

static EXTENSION_ENV: Once = Once::new();

fn test_extension_dir() -> String {
    std::env::var("SIGIRO_EXTENSION_DIR").unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(|home| format!("{home}/.duckdb/extensions"))
            .unwrap_or_else(|_| "/tmp/sigiro-test-extensions".to_string())
    })
}

fn test_pool(dir: &std::path::Path) -> Arc<ProjectPool> {
    EXTENSION_ENV.call_once(|| {
        // SAFETY: all tests in this binary use the same read-only extension cache.
        unsafe {
            std::env::set_var("SIGIRO_EXTENSION_DIR", test_extension_dir());
        }
    });
    Arc::new(ProjectPool::new(
        camino::Utf8PathBuf::from_path_buf(dir.to_path_buf()).unwrap(),
        10,
        sigiro::storage::StorageMode::LocalFile,
    ))
}

// ---------------------------------------------------------------------------
// Helper: build a spans RecordBatch with configurable service_name and span_name
// ---------------------------------------------------------------------------
fn make_spans_batch(n: usize, service_name: &str, span_name: &str, ts_base: i64) -> RecordBatch {
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

    let timestamps: Vec<i64> = (0..n).map(|i| ts_base + i as i64).collect();
    let end_timestamps: Vec<i64> = timestamps.iter().map(|t| t + 5_000_000).collect();
    let durations: Vec<i64> = vec![5_000_000i64; n];
    let trace_ids: Vec<String> = (0..n).map(|i| format!("trace-{i}")).collect();
    let span_ids: Vec<String> = (0..n).map(|i| format!("span-{i}")).collect();
    let service_names: Vec<&str> = vec![service_name; n];
    let span_names: Vec<&str> = vec![span_name; n];
    let nulls: Vec<Option<&str>> = vec![None; n];
    let kind_server: Vec<i32> = vec![2i32; n]; // SERVER
    let status_ok: Vec<i32> = vec![1i32; n]; // OK
    let null_ints: Vec<Option<i32>> = vec![None; n];

    let trace_id_refs: Vec<&str> = trace_ids.iter().map(String::as_str).collect();
    let span_id_refs: Vec<&str> = span_ids.iter().map(String::as_str).collect();

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(TimestampMicrosecondArray::from(timestamps)),
            Arc::new(Int64Array::from(end_timestamps)),
            Arc::new(Int64Array::from(durations)),
            Arc::new(StringArray::from(trace_id_refs)),
            Arc::new(StringArray::from(span_id_refs)),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(service_names)),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(span_names)),
            Arc::new(Int32Array::from(kind_server)),
            Arc::new(Int32Array::from(status_ok)),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(Int32Array::from(null_ints.clone())),
            Arc::new(Int32Array::from(null_ints.clone())),
            Arc::new(Int32Array::from(null_ints.clone())),
            Arc::new(Int32Array::from(null_ints)),
        ],
    )
    .expect("valid spans batch")
}

// ===========================================================================
// Full pipeline — ingest spans via inserter, verify in DuckDB,
//         then run diagnose() and check coverage + operation summaries
// ===========================================================================
#[tokio::test]
async fn e2e_full_pipeline_ingest_and_diagnose() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());
    let router = MsgRouter::new(pool.clone());
    router.start();

    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };

    // Send 5 batches of 20 spans for "checkout" service
    for i in 0..5 {
        let batch = make_spans_batch(20, "checkout", "POST /checkout", now_us + i * 100);
        router
            .send(IngestMsg::Traces {
                project_id: "proj_e2e".to_string(),
                batch,
            })
            .await
            .unwrap();
    }

    router.shutdown_and_wait().await.unwrap();

    // Verify data in DuckDB
    let conn = pool.create_project("proj_e2e").unwrap();
    let count: i64 = conn
        .get()
        .query_row("SELECT count(*) FROM lake.sigiro_spans", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 100, "expected 100 spans ingested, got {count}");

    // Run diagnose()
    let req = DiagnoseRequest {
        services: DiagnoseServices::try_new(vec!["checkout".to_string()]).unwrap(),
        from_ts: Some(sigiro::TimestampMicros::try_new(now_us - 60_000_000).unwrap()),
        to_ts: Some(sigiro::TimestampMicros::try_new(now_us + 60_000_000).unwrap()),
    };
    let reader = conn.get();
    let blocks = diagnose(&reader, &req, "lake.").expect("diagnose should succeed");
    let block = &blocks.results["checkout"];

    // Coverage
    assert!(block.coverage.has_spans, "should have spans");
    assert!(!block.coverage.has_logs, "no logs ingested");
    assert!(!block.coverage.has_metrics, "no metrics ingested");
    assert_eq!(block.coverage.span_count, 100);

    // Operation summaries
    assert_eq!(block.operation_summaries.len(), 1);
    assert_eq!(block.operation_summaries[0].span_name, "POST /checkout");
    assert_eq!(block.operation_summaries[0].count, 100);

    // Service context
    assert_eq!(block.context.service_name, "checkout");
}
