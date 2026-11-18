use arrow::array::{ArrayRef, Int32Array, Int64Array, StringArray, TimestampMicrosecondArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use duckdb::Connection;
use std::sync::Arc;

/// Build a RecordBatch matching the full sigiro_spans schema (25 columns).
fn make_span_batch(n: usize) -> RecordBatch {
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    let schema = Arc::new(Schema::new(vec![
        Field::new(
            "timestamp",
            DataType::Timestamp(arrow::datatypes::TimeUnit::Microsecond, None),
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

    let timestamps: Vec<i64> = (0..n).map(|i| now_us + i as i64).collect();
    let trace_ids: Vec<Option<String>> = (0..n).map(|i| Some(format!("trace-{i}"))).collect();
    let span_ids: Vec<Option<String>> = (0..n).map(|i| Some(format!("span-{i}"))).collect();
    let services: Vec<&str> = vec!["bench-svc"; n];
    let ops: Vec<&str> = (0..n)
        .map(|i| match i % 3 {
            0 => "GET /api",
            1 => "POST /data",
            _ => "PUT /item",
        })
        .collect();
    let pods: Vec<Option<String>> = (0..n).map(|i| Some(format!("pod-{}", i % 5))).collect();
    let attrs: Vec<Option<String>> = (0..n)
        .map(|_| Some("{\"k8s.namespace.name\":\"prod\"}".to_string()))
        .collect();
    let null_strs: Vec<Option<&str>> = vec![None; n];
    let status_codes: Vec<i32> = (0..n).map(|i| if i % 7 == 0 { 2 } else { 1 }).collect();
    let zeros: Vec<i32> = vec![0; n];

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(TimestampMicrosecondArray::from(timestamps)) as ArrayRef,
            Arc::new(Int64Array::from((0..n as i64).collect::<Vec<_>>())),
            Arc::new(Int64Array::from(
                (0..n as i64).map(|i| (i + 1) * 1000).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(trace_ids)),
            Arc::new(StringArray::from(span_ids)),
            Arc::new(StringArray::from(null_strs.clone())), // parent_span_id
            Arc::new(StringArray::from(null_strs.clone())), // trace_state
            Arc::new(StringArray::from(services)),
            Arc::new(StringArray::from(null_strs.clone())), // service_namespace
            Arc::new(StringArray::from(pods)),              // service_instance_id
            Arc::new(StringArray::from(ops)),
            Arc::new(Int32Array::from(vec![2; n])), // span_kind
            Arc::new(Int32Array::from(status_codes)),
            Arc::new(StringArray::from(null_strs.clone())), // status_message
            Arc::new(StringArray::from(attrs)),             // resource_attributes
            Arc::new(StringArray::from(null_strs.clone())), // scope_name
            Arc::new(StringArray::from(null_strs.clone())), // scope_version
            Arc::new(StringArray::from(null_strs.clone())), // scope_attributes
            Arc::new(StringArray::from(null_strs.clone())), // span_attributes
            Arc::new(StringArray::from(null_strs.clone())), // events_json
            Arc::new(StringArray::from(null_strs.clone())), // links_json
            Arc::new(Int32Array::from(zeros.clone())),      // dropped_attributes_count
            Arc::new(Int32Array::from(zeros.clone())),      // dropped_events_count
            Arc::new(Int32Array::from(zeros.clone())),      // dropped_links_count
            Arc::new(Int32Array::from(zeros)),              // flags
        ],
    )
    .expect("valid record batch")
}

/// Setup a DuckLake-backed DB matching production config.
fn setup_ducklake_db() -> (Connection, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let conn = Connection::open_in_memory().expect("in-memory db");
    sigiro::storage::init_default(&conn, camino::Utf8Path::from_path(dir.path()).unwrap())
        .expect("init ducklake");
    sigiro::schema::create_tables(&conn, "lake.").expect("create tables");
    (conn, dir)
}

/// Benchmark: single-batch appender throughput (sigiro_spans).
fn bench_appender_throughput(c: &mut Criterion) {
    let (conn, _dir) = setup_ducklake_db();
    let batch = make_span_batch(10_000);
    let batch_rows = batch.num_rows();

    let mut group = c.benchmark_group("appender");
    group.throughput(Throughput::Elements(batch_rows as u64));
    group.sample_size(10);

    group.bench_function("append_10k_spans", |b| {
        b.iter(|| {
            let mut appender = conn
                .appender_to_catalog_and_db("sigiro_spans", "lake", "main")
                .expect("appender");
            appender.append_record_batch(batch.clone()).expect("append");
            drop(appender);
        });
    });

    group.finish();
}

/// Benchmark: multi-batch flush (matches inserter's flush pattern).
fn bench_appender_multi_batch(c: &mut Criterion) {
    let (conn, _dir) = setup_ducklake_db();
    let batches: Vec<RecordBatch> = (0..5).map(|_| make_span_batch(2_000)).collect();
    let total_rows: usize = batches
        .iter()
        .map(arrow::array::RecordBatch::num_rows)
        .sum();

    let mut group = c.benchmark_group("appender");
    group.throughput(Throughput::Elements(total_rows as u64));
    group.sample_size(10);

    group.bench_function("flush_5x2k_spans", |b| {
        b.iter(|| {
            let mut appender = conn
                .appender_to_catalog_and_db("sigiro_spans", "lake", "main")
                .expect("appender");
            for batch in &batches {
                appender.append_record_batch(batch.clone()).expect("append");
            }
            drop(appender);
        });
    });

    group.finish();
}

/// Benchmark: sustained ingest throughput over time.
fn bench_appender_sustained(c: &mut Criterion) {
    let (conn, _dir) = setup_ducklake_db();
    let batch = make_span_batch(1_000);
    let flushes = 50;
    let total_rows = batch.num_rows() * flushes;

    let mut group = c.benchmark_group("appender");
    group.throughput(Throughput::Elements(total_rows as u64));
    group.sample_size(10);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("sustained_50x1k_spans", |b| {
        b.iter(|| {
            for _ in 0..flushes {
                let mut appender = conn
                    .appender_to_catalog_and_db("sigiro_spans", "lake", "main")
                    .expect("appender");
                appender.append_record_batch(batch.clone()).expect("append");
                drop(appender);
            }
        });
    });

    group.finish();
}

/// Benchmark: plain DuckDB (no DuckLake) as legacy baseline comparison.
/// Plain DuckDB is no longer a storage option in sigiro — this benchmark exists
/// only to measure the overhead DuckLake adds vs raw appender throughput.
fn bench_appender_baseline_duckdb(c: &mut Criterion) {
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch(
        "CREATE TABLE sigiro_spans (
            timestamp             TIMESTAMP NOT NULL,
            end_timestamp         BIGINT NOT NULL,
            duration              BIGINT NOT NULL,
            trace_id              VARCHAR,
            span_id               VARCHAR,
            parent_span_id        VARCHAR,
            trace_state           VARCHAR,
            service_name          VARCHAR NOT NULL,
            service_namespace     VARCHAR,
            service_instance_id   VARCHAR,
            span_name             VARCHAR NOT NULL,
            span_kind             INTEGER NOT NULL,
            status_code           INTEGER NOT NULL,
            status_message        VARCHAR,
            resource_attributes   VARCHAR,
            scope_name            VARCHAR,
            scope_version         VARCHAR,
            scope_attributes      VARCHAR,
            span_attributes       VARCHAR,
            events_json           VARCHAR,
            links_json            VARCHAR,
            dropped_attributes_count INTEGER,
            dropped_events_count  INTEGER,
            dropped_links_count   INTEGER,
            flags                 INTEGER
        )",
    )
    .expect("create table");

    let batch = make_span_batch(10_000);
    let batch_rows = batch.num_rows();

    let mut group = c.benchmark_group("appender");
    group.throughput(Throughput::Elements(batch_rows as u64));
    group.sample_size(10);

    group.bench_function("baseline_10k_spans_plain_duckdb", |b| {
        b.iter(|| {
            let mut appender = conn.appender("sigiro_spans").expect("appender");
            appender.append_record_batch(batch.clone()).expect("append");
            drop(appender);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_appender_throughput,
    bench_appender_multi_batch,
    bench_appender_sustained,
    bench_appender_baseline_duckdb,
);
criterion_main!(benches);
