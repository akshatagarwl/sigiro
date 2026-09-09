use criterion::{Criterion, criterion_group, criterion_main};
use parking_lot::Mutex;
use sigiro::diagnose::{DiagnoseRequest, compute_coverage, diagnose};
use std::sync::Arc;

/// Setup a DuckLake-backed DB matching production config.
fn setup_populated_db() -> (Arc<Mutex<duckdb::Connection>>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");

    // DuckLake file catalog — matches production default
    sigiro::storage::init_default(&conn, camino::Utf8Path::from_path(dir.path()).unwrap())
        .expect("init ducklake");
    sigiro::schema::create_tables(&conn, "lake.").expect("create tables");

    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };

    // Insert 100 spans across 5 operations
    let mut sql = String::from(
        "INSERT INTO lake.sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id,
            parent_span_id, span_name, span_kind, status_code, service_name,
            service_instance_id, resource_attributes) VALUES ",
    );
    for i in 0..100 {
        if i > 0 {
            sql.push(',');
        }
        let op = match i % 5 {
            0 => "POST /checkout",
            1 => "GET /health",
            2 => "POST /pay",
            3 => "GET /products",
            _ => "PUT /cart",
        };
        let status = if i % 7 == 0 { 2 } else { 1 };
        let dur = (i + 1) * 1_000_000;
        let ts = now_us - (100 - i) * 1_000_000;
        sql.push_str(&format!(
            "(make_timestamp({ts}), {ts}+{dur}, {dur}, 'trace-{i}', 'span-{i}', '', '{op}', 2, {status}, 'bench-svc', 'pod-{pod}', '{{\"k8s.namespace.name\":\"prod\"}}')",
            pod = i % 3
        ));
    }
    conn.execute_batch(&sql).expect("insert spans");

    // Insert 50 logs with template-friendly patterns
    let mut log_sql = String::from(
        "INSERT INTO lake.sigiro_logs (timestamp, observed_timestamp, service_name, severity_number, severity_text, body) VALUES ",
    );
    for i in 0..50 {
        if i > 0 {
            log_sql.push(',');
        }
        let ts = now_us - (50 - i) * 1_000_000;
        let body = match i % 4 {
            0 => format!("Connection timeout for 10.0.0.{i}"),
            1 => format!("Failed to process order {i:08x}-e29b-41d4-a716-446655440000"),
            2 => format!("Request completed in {}ms", i * 100 + 50),
            _ => format!("Health check passed for pod-{i}"),
        };
        let sev = if i % 4 == 0 { "WARN" } else { "INFO" };
        let sev_num = if i % 4 == 0 { 13 } else { 9 };
        let body = body.replace('\'', "''");
        log_sql.push_str(&format!(
            "(make_timestamp({ts}), {ts}, 'bench-svc', {sev_num}, '{sev}', '{body}')"
        ));
    }
    conn.execute_batch(&log_sql).expect("insert logs");

    (Arc::new(Mutex::new(conn)), dir)
}

fn bench_diagnose(c: &mut Criterion) {
    let (shared, _dir) = setup_populated_db();
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };

    let mut group = c.benchmark_group("diagnose");
    group.sample_size(10);

    group.bench_function("diagnose_full", |b| {
        b.iter(|| {
            let req = DiagnoseRequest {
                service: Some("bench-svc".to_string()),
                services: None,
                from_ts: Some(
                    sigiro::TimestampMicros::try_new(now_us - 15 * 60 * 1_000_000).unwrap(),
                ),
                to_ts: Some(sigiro::TimestampMicros::try_new(now_us + 60 * 1_000_000).unwrap()),
            };
            diagnose(&shared.lock(), &req, "lake.").expect("diagnose");
        });
    });

    group.finish();
}

fn bench_compute_coverage(c: &mut Criterion) {
    c.bench_function("compute_coverage", |b| {
        b.iter(|| compute_coverage(100, 50, 10, 0));
    });
}

criterion_group!(benches, bench_diagnose, bench_compute_coverage);
criterion_main!(benches);
