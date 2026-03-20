use criterion::{Criterion, criterion_group, criterion_main};
use sigiro::investigate::{
    InvestigateRequest, adaptive_config, build_span_trees, compute_coverage, investigate,
    query_logs, query_service_context,
};
use std::sync::{Arc, Mutex};

fn setup_populated_db() -> Arc<Mutex<duckdb::Connection>> {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn).expect("create tables");

    let now_us = chrono::Utc::now().timestamp_micros();

    // Insert 100 spans across 5 operations, mix of errors and ok
    let mut sql = String::from(
        "INSERT INTO sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id,
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

    // Insert 50 logs with some duplicatable patterns
    let mut log_sql = String::from(
        "INSERT INTO sigiro_logs (timestamp, observed_timestamp, service_name, severity_number, severity_text, body) VALUES ",
    );
    for i in 0..50 {
        if i > 0 {
            log_sql.push(',');
        }
        let ts = now_us - (50 - i) * 1_000_000;
        let body = match i % 4 {
            0 => format!("Connection timeout for 10.0.0.{i}"),
            1 => format!("Failed to process order {i:08x}-e29b-41d4-a716-446655440000"),
            2 => format!("Request completed in {dur}ms", dur = i * 100 + 50),
            _ => format!("Health check passed for pod-{i}"),
        };
        let sev = if i % 4 == 0 { "WARN" } else { "INFO" };
        let sev_num = if i % 4 == 0 { 13 } else { 9 };
        // Escape single quotes in body
        let body = body.replace('\'', "''");
        log_sql.push_str(&format!(
            "(make_timestamp({ts}), {ts}, 'bench-svc', {sev_num}, '{sev}', '{body}')"
        ));
    }
    conn.execute_batch(&log_sql).expect("insert logs");

    Arc::new(Mutex::new(conn))
}

fn bench_investigate(c: &mut Criterion) {
    let shared = setup_populated_db();
    let now_us = chrono::Utc::now().timestamp_micros();

    c.bench_function("investigate_full", |b| {
        b.iter(|| {
            let req = InvestigateRequest {
                service: "bench-svc".to_string(),
                from_ts: Some(now_us - 15 * 60 * 1_000_000),
                to_ts: Some(now_us + 60 * 1_000_000),
            };
            investigate(&shared, &req).expect("investigate");
        });
    });
}

fn bench_compute_coverage(c: &mut Criterion) {
    c.bench_function("compute_coverage", |b| {
        b.iter(|| compute_coverage(100, 50, 10));
    });
}

fn bench_query_logs(c: &mut Criterion) {
    let shared = setup_populated_db();
    let now_us = chrono::Utc::now().timestamp_micros();
    let conn = shared.lock().unwrap();

    c.bench_function("query_logs_regex", |b| {
        b.iter(|| {
            query_logs(
                &conn,
                "bench-svc",
                now_us - 15 * 60 * 1_000_000,
                now_us + 60 * 1_000_000,
                30,
            )
            .expect("query_logs");
        });
    });
}

fn bench_build_span_trees(c: &mut Criterion) {
    use sigiro::investigate::SpanRow;

    let spans: Vec<SpanRow> = (0..100)
        .map(|i| SpanRow {
            trace_id: format!("trace-{}", i / 5),
            span_id: format!("span-{i}"),
            parent_span_id: if i % 5 == 0 {
                String::new()
            } else {
                format!("span-{}", i - (i % 5))
            },
            span_name: "op".to_string(),
            duration_ns: 1_000_000,
            status_code: 1,
            service_name: "svc".to_string(),
        })
        .collect();

    c.bench_function("build_span_trees_100", |b| {
        b.iter(|| build_span_trees(&spans));
    });
}

fn bench_adaptive_config(c: &mut Criterion) {
    let now = 1_700_000_000_000_000_i64;
    c.bench_function("adaptive_config", |b| {
        b.iter(|| adaptive_config(now - 15 * 60 * 1_000_000, now));
    });
}

fn bench_service_context(c: &mut Criterion) {
    let shared = setup_populated_db();
    let now_us = chrono::Utc::now().timestamp_micros();
    let conn = shared.lock().unwrap();

    c.bench_function("query_service_context", |b| {
        b.iter(|| {
            query_service_context(
                &conn,
                "bench-svc",
                now_us - 15 * 60 * 1_000_000,
                now_us + 60 * 1_000_000,
            )
            .expect("context");
        });
    });
}

criterion_group!(
    benches,
    bench_investigate,
    bench_compute_coverage,
    bench_query_logs,
    bench_build_span_trees,
    bench_adaptive_config,
    bench_service_context,
);
criterion_main!(benches);
