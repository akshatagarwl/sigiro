//! End-to-end tests for the sigiro observability pipeline.
//!
//! These tests verify the full OTLP ingest → DuckDB → investigate path,
//! multi-project isolation, services endpoint simulation, query validation,
//! and investigation with all signal types.

use arrow::array::{Float64Array, Int32Array, Int64Array, StringArray, TimestampMicrosecondArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use crossbeam_channel::unbounded;
use sigiro::ingest::IngestMsg;
use sigiro::investigate::{InvestigateRequest, investigate};
use sigiro::pool::ProjectPool;
use sigiro::server::validate_read_only;
use std::sync::Arc;

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

// ===========================================================================
// TEST 1: Full pipeline — ingest spans via inserter, verify in DuckDB,
//         then run investigate() and check coverage + operation summaries
// ===========================================================================
#[test]
fn e2e_full_pipeline_ingest_and_investigate() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(dir.path().to_path_buf(), "", 10));
    let (tx, rx) = unbounded();

    let pool_clone = Arc::clone(&pool);
    let handle = std::thread::spawn(move || {
        sigiro::inserter::run_inserter(&pool_clone, rx).unwrap();
    });

    let now_us = chrono::Utc::now().timestamp_micros();

    // Send 5 batches of 20 spans for "checkout" service
    for i in 0..5 {
        let batch = make_spans_batch(20, "checkout", "POST /checkout", now_us + i * 100);
        tx.send(IngestMsg::Traces {
            project_id: "proj_e2e".to_string(),
            batch,
        })
        .unwrap();
    }

    drop(tx);
    handle.join().unwrap();

    // Verify data in DuckDB
    let conn = pool.get_or_create("proj_e2e").unwrap();
    let count: i64 = conn
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM sigiro_spans", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 100, "expected 100 spans ingested, got {count}");

    // Run investigate()
    let req = InvestigateRequest {
        service: "checkout".to_string(),
        from_ts: Some(now_us - 60_000_000),
        to_ts: Some(now_us + 60_000_000),
    };
    let block = investigate(&conn, &req, "").expect("investigate should succeed");

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
//         investigate on A should not see B's data
// ===========================================================================
#[test]
fn e2e_multi_project_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(dir.path().to_path_buf(), "", 10));
    let (tx, rx) = unbounded();

    let pool_clone = Arc::clone(&pool);
    let handle = std::thread::spawn(move || {
        sigiro::inserter::run_inserter(&pool_clone, rx).unwrap();
    });

    let now_us = chrono::Utc::now().timestamp_micros();

    // Project A: "alpha-svc" with 30 spans
    tx.send(IngestMsg::Traces {
        project_id: "proj_alpha".to_string(),
        batch: make_spans_batch(30, "alpha-svc", "GET /alpha", now_us),
    })
    .unwrap();

    // Project B: "beta-svc" with 50 spans
    tx.send(IngestMsg::Traces {
        project_id: "proj_beta".to_string(),
        batch: make_spans_batch(50, "beta-svc", "GET /beta", now_us),
    })
    .unwrap();

    drop(tx);
    handle.join().unwrap();

    // Investigate project A — should only see alpha-svc
    let conn_a = pool.get_or_create("proj_alpha").unwrap();
    let req_a = InvestigateRequest {
        service: "alpha-svc".to_string(),
        from_ts: Some(now_us - 60_000_000),
        to_ts: Some(now_us + 60_000_000),
    };
    let block_a = investigate(&conn_a, &req_a, "").unwrap();
    assert_eq!(block_a.coverage.span_count, 30);
    assert_eq!(block_a.operation_summaries.len(), 1);
    assert_eq!(block_a.operation_summaries[0].span_name, "GET /alpha");

    // Project A should NOT see beta-svc
    let req_beta_on_a = InvestigateRequest {
        service: "beta-svc".to_string(),
        from_ts: Some(now_us - 60_000_000),
        to_ts: Some(now_us + 60_000_000),
    };
    let block_beta_on_a = investigate(&conn_a, &req_beta_on_a, "").unwrap();
    assert_eq!(
        block_beta_on_a.coverage.span_count, 0,
        "beta-svc should NOT appear in project alpha's DB"
    );

    // Investigate project B — should only see beta-svc
    let conn_b = pool.get_or_create("proj_beta").unwrap();
    let req_b = InvestigateRequest {
        service: "beta-svc".to_string(),
        from_ts: Some(now_us - 60_000_000),
        to_ts: Some(now_us + 60_000_000),
    };
    let block_b = investigate(&conn_b, &req_b, "").unwrap();
    assert_eq!(block_b.coverage.span_count, 50);
    assert_eq!(block_b.operation_summaries[0].span_name, "GET /beta");
}

// ===========================================================================
// TEST 3: Services endpoint simulation — 3 services ingested, query grouped
//         by service_name to verify all appear with correct counts
// ===========================================================================
#[test]
fn e2e_services_endpoint_simulation() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(dir.path().to_path_buf(), "", 10));
    let (tx, rx) = unbounded();

    let pool_clone = Arc::clone(&pool);
    let handle = std::thread::spawn(move || {
        sigiro::inserter::run_inserter(&pool_clone, rx).unwrap();
    });

    let now_us = chrono::Utc::now().timestamp_micros();

    // Ingest spans for 3 different services into the same project
    tx.send(IngestMsg::Traces {
        project_id: "proj_services".to_string(),
        batch: make_spans_batch(10, "api-gateway", "GET /route", now_us),
    })
    .unwrap();
    tx.send(IngestMsg::Traces {
        project_id: "proj_services".to_string(),
        batch: make_spans_batch(25, "user-service", "GET /users", now_us + 100),
    })
    .unwrap();
    tx.send(IngestMsg::Traces {
        project_id: "proj_services".to_string(),
        batch: make_spans_batch(15, "order-service", "POST /orders", now_us + 200),
    })
    .unwrap();

    drop(tx);
    handle.join().unwrap();

    // Query the services table grouped by service_name
    let conn = pool.get_or_create("proj_services").unwrap();
    let conn_guard = conn.lock().unwrap();

    let mut stmt = conn_guard
        .prepare(
            "SELECT service_name, count(*) AS cnt
             FROM sigiro_spans
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

    // Allowed: complex CTE
    assert!(
        validate_read_only(
            "WITH error_spans AS (
            SELECT trace_id, span_name, duration
            FROM sigiro_spans
            WHERE status_code = 2
        )
        SELECT trace_id, span_name, duration
        FROM error_spans
        ORDER BY duration DESC
        LIMIT 10"
        )
        .is_ok()
    );

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
// TEST 5: Investigation with all signals — spans + logs + metrics (gauge),
//         verify coverage shows all three present with no gaps
// ===========================================================================
#[test]
fn e2e_investigation_all_signals() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(dir.path().to_path_buf(), "", 10));
    let (tx, rx) = unbounded();

    let pool_clone = Arc::clone(&pool);
    let handle = std::thread::spawn(move || {
        sigiro::inserter::run_inserter(&pool_clone, rx).unwrap();
    });

    let now_us = chrono::Utc::now().timestamp_micros();

    // Ingest spans
    tx.send(IngestMsg::Traces {
        project_id: "proj_all_signals".to_string(),
        batch: make_spans_batch(10, "full-svc", "GET /data", now_us),
    })
    .unwrap();

    // Ingest logs
    tx.send(IngestMsg::Logs {
        project_id: "proj_all_signals".to_string(),
        batch: make_logs_batch(5, "full-svc", now_us),
    })
    .unwrap();

    // Ingest gauge metrics
    let gauge_batch = make_gauge_batch(3, "full-svc", now_us);
    tx.send(IngestMsg::Metrics {
        project_id: "proj_all_signals".to_string(),
        batches: otlp2records::MetricBatches {
            gauge: Some(gauge_batch),
            sum: None,
            histogram: None,
            exp_histogram: None,
            skipped: Default::default(),
        },
    })
    .unwrap();

    drop(tx);
    handle.join().unwrap();

    // Verify raw counts in DuckDB
    let conn = pool.get_or_create("proj_all_signals").unwrap();
    {
        let guard = conn.lock().unwrap();
        let spans: i64 = guard
            .query_row("SELECT count(*) FROM sigiro_spans", [], |r| r.get(0))
            .unwrap();
        let logs: i64 = guard
            .query_row("SELECT count(*) FROM sigiro_logs", [], |r| r.get(0))
            .unwrap();
        let gauges: i64 = guard
            .query_row("SELECT count(*) FROM sigiro_metrics_gauge", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(spans, 10, "expected 10 spans");
        assert_eq!(logs, 5, "expected 5 logs");
        assert_eq!(gauges, 3, "expected 3 gauge metrics");
    }

    // Run investigate and verify coverage
    let req = InvestigateRequest {
        service: "full-svc".to_string(),
        from_ts: Some(now_us - 60_000_000),
        to_ts: Some(now_us + 60_000_000),
    };
    let block = investigate(&conn, &req, "").expect("investigate with all signals");

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

    // When all 3 signals are present, there should be no gaps
    assert!(
        block.coverage.gaps.is_empty(),
        "no gaps expected when all signals present, got: {:?}",
        block.coverage.gaps
    );

    // Operation summaries should exist for the span data
    assert_eq!(block.operation_summaries.len(), 1);
    assert_eq!(block.operation_summaries[0].span_name, "GET /data");
    assert_eq!(block.operation_summaries[0].count, 10);

    // Logs should be present
    assert!(!block.logs.is_empty(), "should have log templates");

    // Metrics should be present
    assert!(!block.metrics.is_empty(), "should have metric buckets");
}
