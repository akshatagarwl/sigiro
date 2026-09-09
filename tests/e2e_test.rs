//! End-to-end tests for the sigiro observability pipeline.
//!
//! These tests verify the full OTLP ingest → DuckDB → diagnose path,
//! multi-project isolation, services endpoint simulation, query validation,
//! and diagnosis with all signal types.

#![allow(clippy::disallowed_methods)]
// Test harnesses read and write the process environment directly to drive
// storage/config permutations. `clippy.toml` bans that in production code so
// every reader goes through typed-env; a test-only knob routed through
// typed-env would add a production config entry production never reads.

use arrow::array::{
    Float64Array, Int32Array, Int64Array, ListBuilder, StringArray, StringBuilder,
    TimestampMicrosecondArray,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use sigiro::IngestMsg;
use sigiro::diagnose::{DiagnoseRequest, diagnose};
use sigiro::inserter::MsgRouter;
use sigiro::otlp::MetricBatches;
use sigiro::pool::ProjectPool;
use sigiro::sql::validate_read_only;
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

// ---------------------------------------------------------------------------
// Helper: build a logs RecordBatch
// ---------------------------------------------------------------------------
fn make_logs_batch(n: usize, service_name: &str, ts_base: i64) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![
        Field::new(
            "timestamp",
            DataType::Timestamp(TimeUnit::Microsecond, None),
            false,
        ),
        Field::new("observed_timestamp", DataType::Int64, false),
        Field::new("trace_id", DataType::Utf8, true),
        Field::new("span_id", DataType::Utf8, true),
        Field::new("event_name", DataType::Utf8, false),
        Field::new("service_name", DataType::Utf8, false),
        Field::new("service_namespace", DataType::Utf8, true),
        Field::new("service_instance_id", DataType::Utf8, true),
        Field::new("severity_number", DataType::Int32, false),
        Field::new("severity_text", DataType::Utf8, false),
        Field::new("body", DataType::Utf8, true),
        Field::new("resource_attributes", DataType::Utf8, true),
        Field::new("scope_name", DataType::Utf8, true),
        Field::new("scope_version", DataType::Utf8, true),
        Field::new("scope_attributes", DataType::Utf8, true),
        Field::new("log_attributes", DataType::Utf8, true),
    ]));

    let timestamps: Vec<i64> = (0..n).map(|i| ts_base + i as i64).collect();
    let observed: Vec<i64> = timestamps.clone();
    let service_names: Vec<&str> = vec![service_name; n];
    let severity_numbers: Vec<i32> = vec![9i32; n]; // INFO
    let severity_texts: Vec<&str> = vec!["INFO"; n];
    let bodies: Vec<String> = (0..n).map(|i| format!("log message {i}")).collect();
    let body_refs: Vec<&str> = bodies.iter().map(String::as_str).collect();
    let nulls: Vec<Option<&str>> = vec![None; n];

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(TimestampMicrosecondArray::from(timestamps)),
            Arc::new(Int64Array::from(observed)),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(vec![""; n])),
            Arc::new(StringArray::from(service_names)),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(Int32Array::from(severity_numbers)),
            Arc::new(StringArray::from(severity_texts)),
            Arc::new(StringArray::from(body_refs)),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls)),
        ],
    )
    .expect("valid logs batch")
}

// ---------------------------------------------------------------------------
// Helper: build a gauge metrics RecordBatch
// ---------------------------------------------------------------------------
fn make_gauge_batch(n: usize, service_name: &str, ts_base: i64) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![
        Field::new(
            "timestamp",
            DataType::Timestamp(TimeUnit::Microsecond, None),
            false,
        ),
        Field::new("start_timestamp", DataType::Int64, true),
        Field::new("metric_name", DataType::Utf8, false),
        Field::new("metric_description", DataType::Utf8, true),
        Field::new("metric_unit", DataType::Utf8, true),
        Field::new("value", DataType::Float64, false),
        Field::new("service_name", DataType::Utf8, false),
        Field::new("service_namespace", DataType::Utf8, true),
        Field::new("service_instance_id", DataType::Utf8, true),
        Field::new("resource_attributes", DataType::Utf8, true),
        Field::new("scope_name", DataType::Utf8, true),
        Field::new("scope_version", DataType::Utf8, true),
        Field::new("scope_attributes", DataType::Utf8, true),
        Field::new("metric_attributes", DataType::Utf8, true),
        Field::new("flags", DataType::Int32, true),
        Field::new("exemplars_json", DataType::Utf8, true),
    ]));

    let timestamps: Vec<i64> = (0..n).map(|i| ts_base + i as i64).collect();
    let values: Vec<f64> = (0..n).map(|i| 42.0 + i as f64).collect();
    let service_names: Vec<&str> = vec![service_name; n];
    let metric_names: Vec<&str> = vec!["cpu.usage"; n];
    let nulls: Vec<Option<&str>> = vec![None; n];
    let null_ints: Vec<Option<i32>> = vec![None; n];
    let null_i64s: Vec<Option<i64>> = vec![None; n];

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(TimestampMicrosecondArray::from(timestamps)),
            Arc::new(Int64Array::from(null_i64s)),
            Arc::new(StringArray::from(metric_names)),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(Float64Array::from(values)),
            Arc::new(StringArray::from(service_names)),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(Int32Array::from(null_ints)),
            Arc::new(StringArray::from(nulls)),
        ],
    )
    .expect("valid gauge batch")
}

/// Build a profiles RecordBatch manually
/// Build a profiles RecordBatch manually (simulating what transform_profiles produces).
fn make_profiles_batch(
    service_name: &str,
    ts_base: i64,
    stacks: &[(&[&str], i64, &str, &str)], // (stack_frames, value, profile_type, unit)
) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![
        Field::new("profile_id", DataType::Utf8, true),
        Field::new("trace_id", DataType::Utf8, true),
        Field::new("span_id", DataType::Utf8, true),
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
    let mut trace_ids: Vec<Option<&str>> = Vec::new();
    let mut span_ids: Vec<Option<&str>> = Vec::new();
    let mut service_names = Vec::new();
    let mut timestamps = Vec::new();
    let mut profile_types = Vec::new();
    let mut values = Vec::new();
    let mut units = Vec::new();
    let mut resource_attrs: Vec<Option<&str>> = Vec::new();

    let mut stack_traces = ListBuilder::new(StringBuilder::new()).with_field(Arc::new(Field::new(
        "item",
        DataType::Utf8,
        true,
    )));

    for (i, (frames, value, ptype, unit)) in stacks.iter().enumerate() {
        profile_ids.push("test-profile-001");
        trace_ids.push(None);
        span_ids.push(None);
        service_names.push(service_name);
        timestamps.push(ts_base + i as i64);
        profile_types.push(*ptype);
        values.push(*value);
        units.push(*unit);
        resource_attrs.push(None);

        for frame in *frames {
            stack_traces.values().append_value(*frame);
        }
        stack_traces.append(true);
    }

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(profile_ids)),
            Arc::new(StringArray::from(trace_ids)),
            Arc::new(StringArray::from(span_ids)),
            Arc::new(StringArray::from(service_names)),
            Arc::new(arrow::array::TimestampMicrosecondArray::from(timestamps)),
            Arc::new(StringArray::from(profile_types)),
            Arc::new(stack_traces.finish()),
            Arc::new(Int64Array::from(values)),
            Arc::new(StringArray::from(units)),
            Arc::new(StringArray::from(resource_attrs)),
        ],
    )
    .unwrap()
}

// ===========================================================================
// TEST 1: Full pipeline — ingest spans via inserter, verify in DuckDB,
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
        service: Some("checkout".to_string()),
        services: None,
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

// ===========================================================================
// TEST 2: Multi-project isolation — project A and B ingest independently,
//         diagnose on A should not see B's data
// ===========================================================================
#[tokio::test]
async fn e2e_multi_project_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());
    let router = MsgRouter::new(pool.clone());
    router.start();

    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };

    // Project A: "alpha-svc" with 30 spans
    router
        .send(IngestMsg::Traces {
            project_id: "proj_alpha".to_string(),
            batch: make_spans_batch(30, "alpha-svc", "GET /alpha", now_us),
        })
        .await
        .unwrap();

    // Project B: "beta-svc" with 50 spans
    router
        .send(IngestMsg::Traces {
            project_id: "proj_beta".to_string(),
            batch: make_spans_batch(50, "beta-svc", "GET /beta", now_us),
        })
        .await
        .unwrap();

    router.shutdown_and_wait().await.unwrap();

    // Diagnose project A — should only see alpha-svc
    let conn_a = pool.create_project("proj_alpha").unwrap();
    let req_a = DiagnoseRequest {
        service: Some("alpha-svc".to_string()),
        services: None,
        from_ts: Some(sigiro::TimestampMicros::try_new(now_us - 60_000_000).unwrap()),
        to_ts: Some(sigiro::TimestampMicros::try_new(now_us + 60_000_000).unwrap()),
    };
    let reader_a = conn_a.get();
    let blocks_a = diagnose(&reader_a, &req_a, "lake.").expect("diagnose failed");
    let block_a = &blocks_a.results["alpha-svc"];
    assert_eq!(block_a.coverage.span_count, 30);
    assert_eq!(block_a.operation_summaries.len(), 1);
    assert_eq!(block_a.operation_summaries[0].span_name, "GET /alpha");

    // Project A should NOT see beta-svc
    let req_beta_on_a = DiagnoseRequest {
        service: Some("beta-svc".to_string()),
        services: None,
        from_ts: Some(sigiro::TimestampMicros::try_new(now_us - 60_000_000).unwrap()),
        to_ts: Some(sigiro::TimestampMicros::try_new(now_us + 60_000_000).unwrap()),
    };
    let blocks_beta_on_a = diagnose(&reader_a, &req_beta_on_a, "lake.").expect("diagnose failed");
    let block_beta_on_a = &blocks_beta_on_a.results["beta-svc"];
    assert_eq!(
        block_beta_on_a.coverage.span_count, 0,
        "beta-svc should NOT appear in project alpha's DB"
    );

    // Diagnose project B — should only see beta-svc
    let conn_b = pool.create_project("proj_beta").unwrap();
    let req_b = DiagnoseRequest {
        service: Some("beta-svc".to_string()),
        services: None,
        from_ts: Some(sigiro::TimestampMicros::try_new(now_us - 60_000_000).unwrap()),
        to_ts: Some(sigiro::TimestampMicros::try_new(now_us + 60_000_000).unwrap()),
    };
    let reader_b = conn_b.get();
    let blocks_b = diagnose(&reader_b, &req_b, "lake.").expect("diagnose failed");
    let block_b = &blocks_b.results["beta-svc"];
    assert_eq!(block_b.coverage.span_count, 50);
    assert_eq!(block_b.operation_summaries[0].span_name, "GET /beta");
}

// ===========================================================================
// TEST 3: Services endpoint simulation — 3 services ingested, query grouped
//         by service_name to verify all appear with correct counts
// ===========================================================================
#[tokio::test]
async fn e2e_services_endpoint_simulation() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());
    let router = MsgRouter::new(pool.clone());
    router.start();

    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };

    // Ingest spans for 3 different services into the same project
    router
        .send(IngestMsg::Traces {
            project_id: "proj_services".to_string(),
            batch: make_spans_batch(10, "api-gateway", "GET /route", now_us),
        })
        .await
        .unwrap();
    router
        .send(IngestMsg::Traces {
            project_id: "proj_services".to_string(),
            batch: make_spans_batch(25, "user-service", "GET /users", now_us + 100),
        })
        .await
        .unwrap();
    router
        .send(IngestMsg::Traces {
            project_id: "proj_services".to_string(),
            batch: make_spans_batch(15, "order-service", "POST /orders", now_us + 200),
        })
        .await
        .unwrap();

    router.shutdown_and_wait().await.unwrap();

    // Query the services table grouped by service_name
    let conn = pool.create_project("proj_services").unwrap();
    let conn_guard = conn.get();

    let mut stmt = conn_guard
        .prepare(
            "SELECT service_name, count(*) AS cnt
            FROM lake.sigiro_spans
             GROUP BY service_name
             ORDER BY service_name",
        )
        .unwrap();

    let mut rows = stmt.query([]).unwrap();
    let mut services: Vec<(String, i64)> = Vec::new();
    while let Some(row) = rows.next().unwrap() {
        services.push((
            row.get::<_, String>(0).unwrap(),
            row.get::<_, i64>(1).unwrap(),
        ));
    }

    assert_eq!(
        services.len(),
        3,
        "expected 3 services, got {}",
        services.len()
    );

    // Services are ORDER BY service_name
    assert_eq!(services[0].0, "api-gateway");
    assert_eq!(services[0].1, 10);
    assert_eq!(services[1].0, "order-service");
    assert_eq!(services[1].1, 15);
    assert_eq!(services[2].0, "user-service");
    assert_eq!(services[2].1, 25);
}

// ===========================================================================
// TEST 4: Query validation — validate_read_only blocks DML while allowing
//         SELECT with complex CTEs, JOINs, and WHERE clauses
// ===========================================================================
#[test]
fn e2e_query_validation() {
    // Blocked: DML and DDL
    assert!(validate_read_only("INSERT INTO sigiro_spans VALUES (1)").is_err());
    assert!(validate_read_only("UPDATE sigiro_spans SET status_code = 2").is_err());
    assert!(validate_read_only("DELETE FROM sigiro_spans WHERE 1=1").is_err());
    assert!(validate_read_only("DROP TABLE sigiro_spans").is_err());
    assert!(validate_read_only("ALTER TABLE sigiro_spans ADD COLUMN x INT").is_err());
    assert!(validate_read_only("CREATE TABLE evil (x INT)").is_err());
    assert!(validate_read_only("TRUNCATE sigiro_spans").is_err());
    assert!(validate_read_only("COPY sigiro_spans TO '/tmp/evil.csv'").is_err());

    // Case insensitive blocking
    assert!(validate_read_only("insert into sigiro_spans values (1)").is_err());
    assert!(validate_read_only("Insert Into sigiro_spans values (1)").is_err());

    // Allowed: simple SELECT
    assert!(validate_read_only("SELECT * FROM sigiro_spans").is_ok());

    // Allowed: SELECT with WHERE
    assert!(
        validate_read_only(
            "SELECT * FROM sigiro_spans WHERE service_name = 'api' AND status_code = 2"
        )
        .is_ok()
    );

    // Allowed: complex analytical query (subquery form). CTEs (WITH) are
    // banned by policy — see validate_read_only; use a derived-table subquery.
    assert!(
        validate_read_only(
            "SELECT trace_id, span_name, duration
        FROM (
            SELECT trace_id, span_name, duration
            FROM sigiro_spans
            WHERE status_code = 2
        ) error_spans
        ORDER BY duration DESC
        LIMIT 10"
        )
        .is_ok()
    );

    // Rejected: WITH/CTE clauses are banned by policy (firewall hardening).
    assert!(validate_read_only("WITH e AS (SELECT * FROM sigiro_spans) SELECT * FROM e").is_err());

    // Allowed: JOIN
    assert!(
        validate_read_only(
            "SELECT s.span_name, l.body
         FROM sigiro_spans s
         JOIN sigiro_logs l ON s.trace_id = l.trace_id
         WHERE s.service_name = 'api'"
        )
        .is_ok()
    );

    // Allowed: aggregate with GROUP BY
    assert!(
        validate_read_only(
            "SELECT service_name, count(*), avg(duration)
         FROM sigiro_spans
         GROUP BY service_name
         HAVING count(*) > 10"
        )
        .is_ok()
    );

    // Edge case: column names containing blocked keywords should NOT be blocked
    // e.g. "updated_at" contains "UPDATE" but is not a DML statement
    assert!(validate_read_only("SELECT updated_at FROM sigiro_spans").is_ok());
    assert!(validate_read_only("SELECT dropped_attributes_count FROM sigiro_spans").is_ok());
}

// ===========================================================================
// TEST 5: Diagnosis with all signals — spans + logs + metrics (gauge),
//         verify coverage shows all three present with no gaps
// ===========================================================================
#[tokio::test]
async fn e2e_diagnosis_all_signals() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());
    let router = MsgRouter::new(pool.clone());
    router.start();

    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };

    // Ingest spans
    router
        .send(IngestMsg::Traces {
            project_id: "proj_all_signals".to_string(),
            batch: make_spans_batch(10, "full-svc", "GET /data", now_us),
        })
        .await
        .unwrap();

    // Ingest logs
    router
        .send(IngestMsg::Logs {
            project_id: "proj_all_signals".to_string(),
            batch: make_logs_batch(5, "full-svc", now_us),
        })
        .await
        .unwrap();

    // Ingest gauge metrics
    let gauge_batch = make_gauge_batch(3, "full-svc", now_us);
    router
        .send(IngestMsg::Metrics {
            project_id: "proj_all_signals".to_string(),
            batches: MetricBatches {
                gauge: Some(gauge_batch),
                sum: None,
                histogram: None,
                exp_histogram: None,
                skipped: Default::default(),
            },
        })
        .await
        .unwrap();

    // Ingest profiles (CPU samples) so all 4 signal types are present
    let profile_stacks = vec![
        (
            &["main", "handle_request", "db_query"][..],
            15_000_000i64,
            "cpu",
            "nanoseconds",
        ),
        (
            &["main", "handle_request", "serialize"][..],
            8_000_000,
            "cpu",
            "nanoseconds",
        ),
    ];
    router
        .send(IngestMsg::Profiles {
            project_id: "proj_all_signals".to_string(),
            batch: make_profiles_batch("full-svc", now_us, &profile_stacks),
        })
        .await
        .unwrap();

    router.shutdown_and_wait().await.unwrap();

    // Verify raw counts in DuckDB
    let conn = pool.create_project("proj_all_signals").unwrap();
    {
        let guard = conn.get();
        let spans: i64 = guard
            .query_row("SELECT count(*) FROM lake.sigiro_spans", [], |r| r.get(0))
            .unwrap();
        let logs: i64 = guard
            .query_row("SELECT count(*) FROM lake.sigiro_logs", [], |r| r.get(0))
            .unwrap();
        let gauges: i64 = guard
            .query_row("SELECT count(*) FROM lake.sigiro_metrics_gauge", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(spans, 10, "expected 10 spans");
        assert_eq!(logs, 5, "expected 5 logs");
        assert_eq!(gauges, 3, "expected 3 gauge metrics");
    }

    // Run diagnose and verify coverage
    let req = DiagnoseRequest {
        service: Some("full-svc".to_string()),
        services: None,
        from_ts: Some(sigiro::TimestampMicros::try_new(now_us - 60_000_000).unwrap()),
        to_ts: Some(sigiro::TimestampMicros::try_new(now_us + 60_000_000).unwrap()),
    };
    let reader = conn.get();
    let blocks = diagnose(&reader, &req, "lake.").expect("diagnose with all signals");
    let block = &blocks.results["full-svc"];

    assert!(
        block.coverage.has_spans,
        "coverage should show has_spans=true"
    );
    assert!(
        block.coverage.has_logs,
        "coverage should show has_logs=true"
    );
    assert!(
        block.coverage.has_metrics,
        "coverage should show has_metrics=true"
    );
    assert_eq!(block.coverage.span_count, 10);

    // When all 3 signals are present, there should be no critical gaps
    let critical_gaps: Vec<_> = block
        .coverage
        .signals
        .iter()
        .filter(|s| !s.available && s.severity == "high")
        .collect();
    assert!(
        critical_gaps.is_empty(),
        "no critical gaps expected when all signals present, got: {:?}",
        critical_gaps
    );

    // Operation summaries should exist for the span data
    assert_eq!(block.operation_summaries.len(), 1);
    assert_eq!(block.operation_summaries[0].span_name, "GET /data");
    assert_eq!(block.operation_summaries[0].count, 10);

    // Logs should be present
    assert!(!block.logs.sample.is_empty(), "should have log templates");

    // Metrics should be present
    assert!(!block.metrics.is_empty(), "should have metric buckets");
}

// ---------------------------------------------------------------------------
// DRAIN log template dedup e2e: logs with variable tokens should collapse
// ---------------------------------------------------------------------------

/// Build a logs batch where bodies follow the pattern "Request {id} failed with code 500"
/// with different IDs — DRAIN should collapse these into one template.
fn make_templated_logs_batch(bodies: &[&str], service_name: &str, ts_base: i64) -> RecordBatch {
    let n = bodies.len();
    let schema = Arc::new(Schema::new(vec![
        Field::new(
            "timestamp",
            DataType::Timestamp(TimeUnit::Microsecond, None),
            false,
        ),
        Field::new("observed_timestamp", DataType::Int64, false),
        Field::new("trace_id", DataType::Utf8, true),
        Field::new("span_id", DataType::Utf8, true),
        Field::new("event_name", DataType::Utf8, false),
        Field::new("service_name", DataType::Utf8, false),
        Field::new("service_namespace", DataType::Utf8, true),
        Field::new("service_instance_id", DataType::Utf8, true),
        Field::new("severity_number", DataType::Int32, false),
        Field::new("severity_text", DataType::Utf8, false),
        Field::new("body", DataType::Utf8, true),
        Field::new("resource_attributes", DataType::Utf8, true),
        Field::new("scope_name", DataType::Utf8, true),
        Field::new("scope_version", DataType::Utf8, true),
        Field::new("scope_attributes", DataType::Utf8, true),
        Field::new("log_attributes", DataType::Utf8, true),
    ]));

    let timestamps: Vec<i64> = (0..n).map(|i| ts_base + i as i64).collect();
    let observed: Vec<i64> = timestamps.clone();
    let service_names: Vec<&str> = vec![service_name; n];
    let severity_numbers: Vec<i32> = vec![13; n]; // WARN
    let severity_texts: Vec<&str> = vec!["WARN"; n];
    let nulls: Vec<Option<&str>> = vec![None; n];

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(TimestampMicrosecondArray::from(timestamps)),
            Arc::new(Int64Array::from(observed)),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(vec![""; n])),
            Arc::new(StringArray::from(service_names)),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(Int32Array::from(severity_numbers)),
            Arc::new(StringArray::from(severity_texts)),
            Arc::new(StringArray::from(bodies.to_vec())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls.clone())),
            Arc::new(StringArray::from(nulls)),
        ],
    )
    .unwrap()
}

#[tokio::test]
async fn e2e_drain_log_template_dedup() {
    // Verify the full pipeline: OTLP logs → inserter (DRAIN) → DuckDB → diagnose
    // Logs with variable tokens should collapse into a single template.
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());
    let router = MsgRouter::new(pool.clone());
    router.start();

    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };

    // Send 20 logs that should collapse into 2 templates:
    // Template 1: "Request <*> failed with code 500" (10 logs)
    // Template 2: "User <*> logged in from <*>" (10 logs)
    let request_bodies: Vec<String> = (0..10)
        .map(|i| format!("Request {i} failed with code 500"))
        .collect();
    let login_bodies: Vec<String> = (0..10)
        .map(|i| format!("User user_{i} logged in from 10.0.0.{i}"))
        .collect();

    let req_refs: Vec<&str> = request_bodies.iter().map(String::as_str).collect();
    let login_refs: Vec<&str> = login_bodies.iter().map(String::as_str).collect();

    router
        .send(IngestMsg::Logs {
            project_id: "drain_test".to_string(),
            batch: make_templated_logs_batch(&req_refs, "api-svc", now_us),
        })
        .await
        .unwrap();
    router
        .send(IngestMsg::Logs {
            project_id: "drain_test".to_string(),
            batch: make_templated_logs_batch(&login_refs, "api-svc", now_us + 100),
        })
        .await
        .unwrap();

    router.shutdown_and_wait().await.unwrap();

    // Verify: 20 rows in sigiro_logs, each with a template_id
    let conn = pool.create_project("drain_test").unwrap();
    let conn = conn.get();

    let total_logs: i64 = conn
        .query_row("SELECT count(*) FROM lake.sigiro_logs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total_logs, 20, "should have 20 log rows");

    // All rows should have a template_id (not NULL)
    let null_templates: i64 = conn
        .query_row(
            "SELECT count(*) FROM lake.sigiro_logs WHERE template_id IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(null_templates, 0, "all logs should have template_id");

    // Should have a small number of distinct template_ids (2-3, not 20)
    let distinct_templates: i64 = conn
        .query_row(
            "SELECT count(DISTINCT template_id) FROM lake.sigiro_logs",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        distinct_templates <= 4,
        "DRAIN should collapse 20 logs to ≤4 templates, got {distinct_templates}"
    );

    // Template lookup table should have entries
    let template_count: i64 = conn
        .query_row("SELECT count(*) FROM lake.sigiro_log_templates", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(
        template_count >= 1,
        "sigiro_log_templates should have entries"
    );

    // At least one template should contain a placeholder (<*>, <NUM>, <IP>, etc.)
    let wildcard_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM lake.sigiro_log_templates WHERE template_text LIKE '%<%>%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        wildcard_count >= 1,
        "at least one template should have placeholder wildcards"
    );

    // Verify via diagnose: raw logs come back, and templates dedup to a
    // handful of patterns (not 20 raw lines).
    let req = DiagnoseRequest {
        service: Some("api-svc".to_string()),
        services: None,
        from_ts: Some(sigiro::TimestampMicros::try_new(now_us - 60_000_000).unwrap()),
        to_ts: Some(sigiro::TimestampMicros::try_new(now_us + 60_000_000).unwrap()),
    };
    let blocks = diagnose(&conn, &req, "lake.").unwrap();
    let block = &blocks.results["api-svc"];
    assert!(
        !block.logs.sample.is_empty(),
        "diagnose should return some logs, got empty"
    );

    let patterns = &block.log_patterns;
    assert!(!patterns.is_empty(), "should have log patterns");
    assert!(
        patterns.len() <= 4,
        "20 logs should dedup to ≤4 patterns, got {}",
        patterns.len()
    );
    let login_pattern = patterns
        .iter()
        .find(|p| p.pattern.contains("logged in"))
        .expect("should have login pattern");

    assert!(
        login_pattern.template_id.is_some(),
        "login pattern should have template_id"
    );
    assert!(
        login_pattern.drill_down_sql.is_some(),
        "login pattern should have drill_down_sql"
    );
    assert!(
        login_pattern.count >= 1,
        "login pattern should group at least one log"
    );
    let grouped_total: i64 = patterns.iter().map(|p| p.count).sum();
    assert_eq!(grouped_total, 20, "patterns should account for all 20 logs");
    assert!(
        login_pattern.example_body.is_some(),
        "login pattern should carry an example body"
    );
    // Extracted-variable samples: each is a JSON array of wildcard values
    // ("user_3", "10.0.0.3", ...) — up to 5 distinct tuples.
    let samples = login_pattern
        .sample_values
        .as_ref()
        .and_then(|v| v.as_array())
        .expect("login pattern should have sample_values");
    assert!(
        !samples.is_empty() && samples.len() <= 5,
        "sample_values should hold 1..=5 tuples, got {}",
        samples.len()
    );
    assert!(
        samples.iter().all(serde_json::Value::is_array),
        "each sample is a JSON array of wildcard values"
    );

    // Find the request pattern and verify grouping
    let req_pattern = patterns
        .iter()
        .find(|p| p.pattern.contains("Request"))
        .expect("should have request pattern");
    assert!(
        req_pattern.count >= 1,
        "request pattern should group at least one log"
    );

    // Raw logs are limited (50), not deduplicated
    assert!(
        block.logs.sample.len() <= 50,
        "log count should be ≤50 limit, got {}",
        block.logs.sample.len()
    );
}
