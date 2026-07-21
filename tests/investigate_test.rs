use parking_lot::Mutex;
use sigiro::investigate::{
    InvestigateRequest, compute_coverage, investigate, metric_bucket_interval, us_to_iso,
};
use std::sync::Arc;

/// Helper: run investigate for one service and return its block.
fn investigate_service(
    conn: &duckdb::Connection,
    service: &str,
    from_ts: i64,
    to_ts: i64,
) -> sigiro::investigate::InvestigationBlock {
    let req = InvestigateRequest {
        service: Some(service.to_string()),
        services: None,
        from_ts,
        to_ts,
    };
    let mut blocks = investigate(conn, &req, "").expect("investigate");
    blocks.results.remove(service).expect("block for service")
}

/// Helper: set up in-memory DB with DRAIN-classified logs and deduplicated templates.
/// Returns (shared_conn, now_us).
fn setup_drain_logs(
    prewarm: &[&str],
    bodies: &[&str],
    severity_number: i32,
    severity_text: &str,
) -> (Arc<Mutex<duckdb::Connection>>, i64) {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };

    let mut drain = sigiro::drain::DrainTree::new();
    for example in prewarm {
        drain.classify(example);
    }

    let mut seen_templates: std::collections::HashSet<u64> = std::collections::HashSet::new();
    for body in bodies {
        let (template, id, _) = drain.classify(body);
        conn.execute(
            &format!(
                "INSERT INTO sigiro_logs (timestamp, observed_timestamp, service_name, severity_number,
                    severity_text, body, template_id)
                 VALUES (make_timestamp({now_us}), {now_us}, 'api', {severity_number}, '{severity_text}', ?, ?)"
            ),
            duckdb::params![body, id],
        )
        .expect("insert log");
        if seen_templates.insert(id) {
            conn.execute(
                "INSERT INTO sigiro_log_templates (template_id, template_text) VALUES (?, ?)",
                duckdb::params![id, template],
            )
            .expect("insert template");
        }
    }

    (Arc::new(Mutex::new(conn)), now_us)
}

// Span tree tests removed - trace grouping is now integrated into Trace type

#[test]
fn us_to_iso_valid() {
    let iso = us_to_iso(1_700_000_000_000_000).unwrap();
    assert!(iso.contains("2023-11"));
}

#[test]
fn us_to_iso_zero() {
    let iso = us_to_iso(0).unwrap();
    assert!(iso.contains("1970"));
}

#[test]
fn investigate_with_in_memory_db() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let shared = Arc::new(Mutex::new(conn));
    let req = InvestigateRequest {
        service: Some("nonexistent".to_string()),
        services: None,
        from_ts: 0,
        to_ts: 0,
    };
    let blocks =
        investigate(&shared.lock(), &req, "").expect("investigate should succeed on empty db");
    let block = &blocks.results["nonexistent"];
    assert_eq!(block.service, "nonexistent");
    assert!(block.operation_summaries.is_empty());
    assert!(block.metrics.is_empty());
    assert!(block.spans.sample.is_empty());
    assert!(block.logs.sample.is_empty());
    assert_eq!(block.spans.total, 0);
    assert_eq!(block.logs.total, 0);
    assert_eq!(block.error_traces_total, 0);

    // Coverage: empty db → all gaps present
    assert!(!block.coverage.has_spans);
    assert!(!block.coverage.has_logs);
    assert!(!block.coverage.has_metrics);
    assert_eq!(block.coverage.span_count, 0);
    assert_eq!(block.coverage.log_template_count, 0);
    assert_eq!(block.coverage.metric_series_count, 0);
    assert!(
        block.coverage.signals.len() >= 3,
        "should have gaps for all signal types"
    );
    assert!(
        block
            .coverage
            .signals
            .iter()
            .any(|s| s.reason.contains("no spans"))
    );
    assert!(
        block
            .coverage
            .signals
            .iter()
            .any(|s| s.reason.contains("no logs"))
    );
    assert!(
        block
            .coverage
            .signals
            .iter()
            .any(|s| s.reason.contains("no metrics"))
    );
}

#[test]
fn investigate_with_real_span_data() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");

    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    conn.execute_batch(&format!(
        "INSERT INTO sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id,
            parent_span_id, span_name, span_kind, status_code, service_name)
         VALUES
            (make_timestamp({now_us}), {now_us} + 500000000, 500000000, 'trace-1', 'span-1', '', 'POST /checkout', 2, 2, 'checkout'),
            (make_timestamp({now_us}), {now_us} + 200000000, 200000000, 'trace-2', 'span-2', '', 'POST /checkout', 2, 1, 'checkout'),
            (make_timestamp({now_us}), {now_us} + 300000000, 300000000, 'trace-3', 'span-3', '', 'POST /checkout', 2, 2, 'checkout'),
            (make_timestamp({now_us}), {now_us} + 1000000, 1000000, 'trace-4', 'span-4', '', 'GET /health', 2, 1, 'checkout'),
            (make_timestamp({now_us}), {now_us} + 2000000, 2000000, 'trace-5', 'span-5', '', 'GET /health', 2, 1, 'checkout');"
    )).expect("insert spans");

    let shared = Arc::new(Mutex::new(conn));
    let req = InvestigateRequest {
        service: Some("checkout".to_string()),
        services: None,
        from_ts: now_us - 60_000_000,
        to_ts: now_us + 60_000_000,
    };
    let blocks = investigate(&shared.lock(), &req, "").expect("investigate");
    let block = &blocks.results["checkout"];

    assert_eq!(block.operation_summaries.len(), 2);
    let checkout_op = &block.operation_summaries[0];
    assert_eq!(checkout_op.span_name, "POST /checkout");
    assert_eq!(checkout_op.count, 3);
    assert_eq!(checkout_op.error_count, 2);
    assert!((checkout_op.error_pct - 66.66).abs() < 1.0);
    assert!(checkout_op.exemplar_error_trace_id.is_some());
    assert!(checkout_op.baseline_error_pct.is_none());
    assert!(checkout_op.baseline_p95_us.is_none());
    assert!(checkout_op.baseline_count.is_none());

    let health_op = &block.operation_summaries[1];
    assert_eq!(health_op.span_name, "GET /health");
    assert_eq!(health_op.count, 2);
    assert_eq!(health_op.error_count, 0);
    assert!(health_op.exemplar_error_trace_id.is_none());

    assert_eq!(block.spans.sample.len(), 5);
    assert!(
        block.spans.sample[0].is_error,
        "errors sort first in flat spans"
    );
    assert_eq!(block.spans.total, 5);
    assert_eq!((block.spans.sample.len() as i64), 5);
    assert_eq!(block.error_traces_total, 2);

    // Drill affordances (#1/#2): every lead carries a ready-to-run query.
    assert!(
        checkout_op
            .drill_down_sql
            .as_ref()
            .unwrap()
            .contains("span_name = 'POST /checkout'"),
        "operation drill query targets that operation's raw spans"
    );
    assert!(
        block.spans.sample[0]
            .drill_down_sql
            .as_ref()
            .unwrap()
            .contains("trace_id ="),
        "span drill query fetches the span's full trace"
    );
    assert!(
        block.spans.drill_all_sql.contains("sigiro_spans"),
        "sampled section carries a full-set query"
    );
    assert!(!block.spans.sampled_by.is_empty());

    assert!(block.coverage.has_spans);
    assert!(!block.coverage.has_logs);
    assert!(!block.coverage.has_metrics);
    assert_eq!(block.coverage.span_count, 5);
    assert!(
        block
            .coverage
            .signals
            .iter()
            .any(|s| s.reason.contains("no logs"))
    );
    assert!(
        block
            .coverage
            .signals
            .iter()
            .any(|s| s.reason.contains("spans exist but no logs"))
    );
    assert!(
        !block
            .coverage
            .signals
            .iter()
            .any(|s| s.reason.contains("no spans"))
    );
}

#[test]
fn coverage_no_data() {
    let c = compute_coverage(0, 0, 0, 0);
    assert!(!c.has_spans);
    assert!(!c.has_logs);
    assert!(!c.has_metrics);
    assert!(c.signals.len() >= 3);
}

#[test]
fn coverage_spans_only() {
    let c = compute_coverage(10, 0, 0, 0);
    assert!(c.has_spans);
    assert!(!c.has_logs);
    assert!(!c.has_metrics);
    assert!(
        c.signals
            .iter()
            .any(|s| s.reason.contains("spans exist but no logs"))
    );
    assert!(!c.signals.iter().any(|s| s.reason.contains("no spans")));
}

#[test]
fn coverage_all_signals() {
    let c = compute_coverage(10, 5, 3, 1);
    assert!(c.has_spans);
    assert!(c.has_logs);
    assert!(c.has_metrics);
    assert!(c.has_profiles);
    assert!(
        !c.signals
            .iter()
            .any(|s| !s.available && s.severity == "high"),
        "no critical gaps when all signals present"
    );
}

#[test]
fn service_context_empty_db() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    let ctx = investigate_service(&conn, "nonexistent", now_us - 60_000_000, now_us).context;
    assert_eq!(ctx.service_name, "nonexistent");
    assert_eq!(ctx.pod_count, 0);
    assert!(ctx.dependencies.is_empty());
    assert!(ctx.dependents.is_empty());
    assert!(ctx.namespace.is_none());
}

#[test]
fn service_context_with_k8s_attrs() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    conn.execute_batch(&format!(
        "INSERT INTO sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id,
            parent_span_id, span_name, span_kind, status_code, service_name,
            service_instance_id, resource_attributes)
         VALUES
            (make_timestamp({now_us}), {now_us}+1000, 1000, 't1', 's1', '', 'op', 2, 1, 'checkout',
             'pod-1', '{{\"k8s.namespace.name\":\"prod\",\"k8s.deployment.name\":\"checkout-v2\"}}'),
            (make_timestamp({now_us}), {now_us}+1000, 1000, 't2', 's2', '', 'op', 2, 1, 'checkout',
             'pod-2', '{{\"k8s.namespace.name\":\"prod\",\"k8s.deployment.name\":\"checkout-v2\"}}')"
    ))
    .expect("insert");
    let ctx =
        investigate_service(&conn, "checkout", now_us - 60_000_000, now_us + 60_000_000).context;
    assert_eq!(ctx.namespace, Some("prod".to_string()));
    assert_eq!(ctx.deployment, Some("checkout-v2".to_string()));
    assert_eq!(ctx.pod_count, 2);
}

#[test]
fn service_context_dependencies() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    conn.execute_batch(&format!(
        "INSERT INTO sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id,
            parent_span_id, span_name, span_kind, status_code, service_name)
         VALUES
            (make_timestamp({now_us}), {now_us}+1000, 1000, 'trace-x', 'sx1', '', 'POST /pay', 3, 1, 'checkout'),
            (make_timestamp({now_us}), {now_us}+1000, 1000, 'trace-x', 'sx2', 'sx1', 'process', 2, 1, 'payments')"
    ))
    .expect("insert");
    let ctx =
        investigate_service(&conn, "checkout", now_us - 60_000_000, now_us + 60_000_000).context;
    assert!(
        ctx.dependencies
            .iter()
            .any(|d| d.service_name == "payments"),
        "checkout should depend on payments"
    );
    let ctx2 =
        investigate_service(&conn, "payments", now_us - 60_000_000, now_us + 60_000_000).context;
    assert!(
        ctx2.dependents.iter().any(|d| d.service_name == "checkout"),
        "payments should have checkout as dependent"
    );
}

#[test]
fn derived_events_deploy_detection() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    conn.execute_batch(&format!(
        "INSERT INTO sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id,
            parent_span_id, span_name, span_kind, status_code, service_name, resource_attributes)
         VALUES
            (make_timestamp({now_us} - 60000000), {now_us}, 1000, 't1', 's1', '', 'op', 2, 1, 'api',
             '{{\"service.version\":\"v1.0.0\"}}'),
            (make_timestamp({now_us}), {now_us}+1000, 1000, 't2', 's2', '', 'op', 2, 1, 'api',
             '{{\"service.version\":\"v1.1.0\"}}')"
    ))
    .expect("insert");
    let events =
        investigate_service(&conn, "api", now_us - 120_000_000, now_us + 60_000_000).events;
    let deploys: Vec<_> = events.iter().filter(|e| e.event_type == "deploy").collect();
    assert_eq!(
        deploys.len(),
        2,
        "should detect 2 versions as deploy events"
    );
    assert!(deploys.iter().any(|e| e.description.contains("v1.0.0")));
    assert!(deploys.iter().any(|e| e.description.contains("v1.1.0")));
}

#[test]
fn derived_events_k8s_oomkilled() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    conn.execute_batch(&format!(
        "INSERT INTO sigiro_logs (timestamp, observed_timestamp, service_name, severity_number,
            severity_text, body)
         VALUES
            (make_timestamp({now_us}), {now_us}, 'api', 17, 'ERROR',
             'Container killed due to OOMKilled')"
    ))
    .expect("insert");
    let events = investigate_service(&conn, "api", now_us - 60_000_000, now_us + 60_000_000).events;
    let k8s: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "k8s_event")
        .collect();
    assert_eq!(k8s.len(), 1);
    assert!(k8s[0].description.contains("OOMKilled"));
}

#[test]
fn log_template_dedup_ips() {
    // DRAIN dedup: logs with different IPs should collapse to one template.
    let bodies = [
        "Connection timeout for 192.168.1.1",
        "Connection timeout for 10.0.0.5",
        "Connection timeout for 172.16.0.99",
    ];
    let (shared, now_us) = setup_drain_logs(
        &[
            "Connection timeout for 1.1.1.1",
            "Connection timeout for 2.2.2.2",
        ],
        &bodies,
        13,
        "WARN",
    );

    let conn = shared.lock();
    let block = investigate_service(&conn, "api", now_us - 60_000_000, now_us + 60_000_000);
    assert_eq!(block.logs.sample.len(), 3, "3 raw logs returned");
    assert_eq!(
        block.log_patterns.len(),
        1,
        "3 logs with different IPs collapse to one template pattern"
    );
}

#[test]
fn log_template_dedup_uuids() {
    // DRAIN dedup: logs with different UUIDs should collapse to one template.
    let bodies = [
        "Failed to process order 550e8400-e29b-41d4-a716-446655440000",
        "Failed to process order a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    ];
    let (shared, now_us) = setup_drain_logs(
        &[
            "Failed to process order 00000000-0000-0000-0000-000000000000",
            "Failed to process order 11111111-1111-1111-1111-111111111111",
        ],
        &bodies,
        17,
        "ERROR",
    );

    let conn = shared.lock();
    let block = investigate_service(&conn, "api", now_us - 60_000_000, now_us + 60_000_000);
    assert_eq!(block.logs.sample.len(), 2, "2 raw logs returned");
    assert_eq!(
        block.log_patterns.len(),
        1,
        "2 logs with different UUIDs collapse to one template pattern"
    );
}

#[test]
fn metric_bucket_scales_with_window() {
    let now = 1_000_000_000_000_i64;
    assert_eq!(
        metric_bucket_interval(now - 5 * 60 * 1_000_000, now),
        "1 minute"
    );
    assert_eq!(
        metric_bucket_interval(now - 30 * 60 * 1_000_000, now),
        "5 minutes"
    );
    assert_eq!(
        metric_bucket_interval(now - 3 * 3600 * 1_000_000, now),
        "1 hour"
    );
}

#[test]
fn investigate_large_window_still_returns_spans() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    conn.execute_batch(&format!(
        "INSERT INTO sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id,
            parent_span_id, span_name, span_kind, status_code, service_name)
         VALUES
            (make_timestamp({now_us}), {now_us}+1000, 1000, 't1', 's1', '', 'op', 2, 1, 'svc')"
    ))
    .expect("insert");
    let shared = Arc::new(Mutex::new(conn));
    let req = InvestigateRequest {
        service: Some("svc".to_string()),
        services: None,
        from_ts: now_us - 3 * 3600 * 1_000_000,
        to_ts: now_us + 60_000_000,
    };
    let blocks = investigate(&shared.lock(), &req, "").unwrap();
    let block = &blocks.results["svc"];
    assert!(
        !block.spans.sample.is_empty(),
        ">2h window should still return exemplar spans (flat cap, no more span_limit: 0 cliff)"
    );
    assert!(!block.operation_summaries.is_empty());
}

#[test]
fn investigate_multi_basic() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    conn.execute_batch(&format!(
        "INSERT INTO sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id,
            parent_span_id, span_name, span_kind, status_code, service_name)
         VALUES
            (make_timestamp({now_us}), {now_us}+1000, 1000, 't1', 's1', '', 'op', 2, 1, 'svc-a'),
            (make_timestamp({now_us}), {now_us}+1000, 1000, 't2', 's2', '', 'op', 2, 1, 'svc-b'),
            (make_timestamp({now_us}), {now_us}+1000, 1000, 't3', 's3', '', 'op', 2, 1, 'svc-c')"
    ))
    .expect("insert");
    let shared = Arc::new(Mutex::new(conn));
    let req = InvestigateRequest {
        service: None,
        services: Some(vec!["svc-a".into(), "svc-b".into(), "svc-c".into()]),
        from_ts: now_us - 60_000_000,
        to_ts: now_us + 60_000_000,
    };
    let blocks = investigate(&shared.lock(), &req, "").unwrap();
    assert_eq!(blocks.results.len(), 3);
    assert!(blocks.errors.is_empty());
    assert!(blocks.results.contains_key("svc-a"));
    assert!(blocks.results.contains_key("svc-b"));
    assert!(blocks.results.contains_key("svc-c"));
}

#[test]
fn investigate_multi_cap_enforced() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let shared = Arc::new(Mutex::new(conn));
    let services: Vec<String> = (0..11).map(|i| format!("svc-{i}")).collect();
    let req = InvestigateRequest {
        service: None,
        services: Some(services),
        from_ts: 0,
        to_ts: 0,
    };
    let result = investigate(&shared.lock(), &req, "");
    assert!(result.is_err(), "should reject >10 services");
}

#[test]
fn investigate_multi_empty_rejected() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let shared = Arc::new(Mutex::new(conn));
    let req = InvestigateRequest {
        service: None,
        services: Some(vec![]),
        from_ts: 0,
        to_ts: 0,
    };
    assert!(investigate(&shared.lock(), &req, "").is_err());
}

#[test]
fn derived_events_no_events() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    let events = investigate_service(&conn, "api", now_us - 60_000_000, now_us).events;
    assert!(events.is_empty());
}

#[test]
fn investigate_all_three_signals() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");

    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    conn.execute_batch(&format!(
        "INSERT INTO sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id,
            parent_span_id, span_name, span_kind, status_code, service_name)
         VALUES (make_timestamp({now_us}), {now_us} + 1000, 1000, 't1', 's1', '', 'op', 2, 1, 'svc');
         INSERT INTO sigiro_logs (timestamp, observed_timestamp, service_name, severity_number,
            severity_text, body)
         VALUES (make_timestamp({now_us}), {now_us}, 'svc', 13, 'WARN', 'something happened');
         INSERT INTO sigiro_metrics_gauge (timestamp, metric_name, value, service_name)
         VALUES (make_timestamp({now_us}), 'cpu.usage', 42.0, 'svc');
         INSERT INTO sigiro_profiles (profile_id, service_name, timestamp, profile_type, stack_trace, value, unit)
         VALUES ('p1', 'svc', make_timestamp({now_us}), 'cpu', ['main', 'handler'], 1000, 'samples');"
    ))
    .expect("insert all signals");

    let shared = Arc::new(Mutex::new(conn));
    let req = InvestigateRequest {
        service: Some("svc".to_string()),
        services: None,
        from_ts: now_us - 60_000_000,
        to_ts: now_us + 60_000_000,
    };
    let blocks = investigate(&shared.lock(), &req, "").expect("investigate");
    let block = &blocks.results["svc"];

    // Coverage: all 3 signal types present, no gaps
    assert!(block.coverage.has_spans);
    assert!(block.coverage.has_logs);
    assert!(block.coverage.has_metrics);
    assert!(
        !block
            .coverage
            .signals
            .iter()
            .any(|s| !s.available && s.severity == "high"),
        "no critical gaps when all signals present"
    );

    // Truncation present with correct counts
    assert_eq!(block.spans.total, 1);

    // Operation summaries present
    assert!(!block.operation_summaries.is_empty());

    // Window present with valid ISO timestamps
    assert!(
        block.window.from_iso.contains('T'),
        "from_iso should be ISO format"
    );
    assert!(
        block.window.to_iso.contains('T'),
        "to_iso should be ISO format"
    );
    assert!(block.window.from_ts < block.window.to_ts);
}

#[test]
fn operation_summary_baselines() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");

    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    let baseline_ts = now_us - 10 * 60 * 1_000_000;
    conn.execute_batch(&format!(
        "INSERT INTO sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id,
            parent_span_id, span_name, span_kind, status_code, service_name)
         VALUES
            -- Baseline: healthy
            (make_timestamp({baseline_ts}), {baseline_ts} + 50000000, 50000000, 'bl-1', 'bs-1', '', 'POST /checkout', 2, 1, 'checkout'),
            (make_timestamp({baseline_ts}), {baseline_ts} + 60000000, 60000000, 'bl-2', 'bs-2', '', 'POST /checkout', 2, 1, 'checkout'),
            -- Current: broken
            (make_timestamp({now_us}), {now_us} + 800000000, 800000000, 'cur-1', 'cs-1', '', 'POST /checkout', 2, 2, 'checkout'),
            (make_timestamp({now_us}), {now_us} + 900000000, 900000000, 'cur-2', 'cs-2', '', 'POST /checkout', 2, 2, 'checkout');"
    )).expect("insert spans");

    let shared = Arc::new(Mutex::new(conn));
    let req = InvestigateRequest {
        service: Some("checkout".to_string()),
        services: None,
        from_ts: now_us - 5 * 60 * 1_000_000,
        to_ts: now_us + 60_000_000,
    };
    let blocks = investigate(&shared.lock(), &req, "").expect("investigate");
    let block = &blocks.results["checkout"];

    let op = &block.operation_summaries[0];
    assert_eq!(op.span_name, "POST /checkout");
    assert_eq!(op.count, 2);
    assert_eq!(op.error_count, 2);
    assert!((op.error_pct - 100.0).abs() < 0.01);
    assert!(op.baseline_error_pct.is_some());
    assert!((op.baseline_error_pct.unwrap() - 0.0).abs() < 0.01);
    assert!(op.baseline_count.is_some());
    assert_eq!(op.baseline_count.unwrap(), 2);
    assert!(op.baseline_p95_us.is_some());
    assert!(op.baseline_p95_us.unwrap() < 100_000_000.0);
}

#[test]
fn snapshot_compute_coverage_full() {
    let coverage = compute_coverage(10, 5, 3, 1);
    insta::assert_json_snapshot!(coverage);
}

#[test]
fn snapshot_compute_coverage_empty() {
    let coverage = compute_coverage(0, 0, 0, 0);
    insta::assert_json_snapshot!(coverage);
}

#[test]
fn snapshot_query_service_context_shape() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = 1_700_000_000_000_000_i64;
    conn.execute_batch(&format!(
        "INSERT INTO sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id,
            parent_span_id, span_name, span_kind, status_code, service_name,
            service_instance_id, resource_attributes)
         VALUES
            (make_timestamp({now_us}), {now_us}+1000, 1000, 'trace-x', 'sx1', '', 'POST /pay', 3, 1, 'checkout',
             'pod-1', '{{\"k8s.namespace.name\":\"prod\",\"k8s.deployment.name\":\"checkout-v2\"}}'),
            (make_timestamp({now_us}), {now_us}+1000, 1000, 'trace-x', 'sx2', 'sx1', 'process', 2, 1, 'payments',
             'pod-2', '{{\"k8s.namespace.name\":\"prod\",\"k8s.deployment.name\":\"payments-v1\"}}')"
    ))
    .expect("insert");
    let ctx =
        investigate_service(&conn, "checkout", now_us - 60_000_000, now_us + 60_000_000).context;
    insta::assert_json_snapshot!(ctx);
}

// ---------------------------------------------------------------------------
// Continuous anomaly subsystem: the scheduled pass (sigiro::anomaly) writes
// the sigiro_anomalies table; investigate READS it (no inline detection).
// E2E-style: seed spans → run the pass → assert persisted rows → assert
// investigate surfaces them.
// ---------------------------------------------------------------------------

/// Bulk-insert spans for `service`/`span_name` at `ts_us`: `counts` is
/// (total, errors) — the first `errors` spans get status_code=2 — each with
/// the given duration (µs).
fn insert_spans(
    conn: &duckdb::Connection,
    service: &str,
    span_name: &str,
    ts_us: i64,
    counts: (usize, usize),
    duration_us: i64,
    attrs: Option<&str>,
) {
    let (n, errors) = counts;
    let mut values = Vec::with_capacity(n);
    for i in 0..n {
        let status = if i < errors { 2 } else { 1 };
        let attrs_sql = match attrs {
            Some(a) => format!("'{a}'"),
            None => "NULL".to_string(),
        };
        values.push(format!(
            "(make_timestamp({ts_us}), {ts_us} + {duration_us}, {duration_us}, \
             'tr-{span_name}-{ts_us}-{i}', 'sp-{ts_us}-{i}', '', '{span_name}', 2, {status}, \
             '{service}', {attrs_sql})"
        ));
    }
    conn.execute_batch(&format!(
        "INSERT INTO sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id,
            parent_span_id, span_name, span_kind, status_code, service_name, span_attributes)
         VALUES {}",
        values.join(",\n")
    ))
    .expect("insert spans");
}

/// Run the scheduled pass end to end, exactly as production does: `detect`
/// (in production: a READER connection — the heavy scan + BOCPD) then
/// `persist` (in production: the writer — the brief DELETE+INSERT). Tests
/// use one connection for both phases; the code path is identical. Returns
/// the number of rows persisted.
fn run_anomaly_pass(conn: &duckdb::Connection) -> usize {
    let inserts = sigiro::anomaly::detect(conn, "").expect("anomaly detect");
    sigiro::anomaly::persist(conn, "", &inserts).expect("anomaly persist")
}

/// Seed the canonical worsening-shift fixture: flat 0% errors for minutes
/// -74..-16, then 50% errors for minutes -12..-1 — a regime shift ~12
/// minutes before `now_us`.
fn seed_worsening_shift(conn: &duckdb::Connection, now_us: i64) {
    // One batch per distinct 5-minute bucket (the pass buckets at 5min): ~30
    // healthy buckets (0% errors) then ~10 worsening buckets (50%) → a clear
    // sustained regime shift ~50 min ago, inside a 60-min investigate window.
    let bucket = 5 * 60_000_000_i64;
    for i in 11..=40_i64 {
        insert_spans(
            conn,
            "worker",
            "call upstream",
            now_us - i * bucket,
            (10, 0),
            10_000,
            None,
        );
    }
    for i in 1..=10_i64 {
        insert_spans(
            conn,
            "worker",
            "call upstream",
            now_us - i * bucket,
            (10, 5),
            10_000,
            None,
        );
    }
}

#[test]
fn anomaly_pass_persists_worsening_shift_and_investigate_surfaces_it() {
    // The full spine, end to end: seed a worsening error-rate shift, run the
    // scheduled pass, assert a row LANDS IN sigiro_anomalies, then assert
    // /investigate (which only reads the table) flags the operation.
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    seed_worsening_shift(&conn, now_us);

    let written = run_anomaly_pass(&conn);
    assert!(written >= 1, "the shift must persist at least one row");

    // The persisted row carries the full contract.
    let (service, operation, signal, kind, direction, before, after, drill): (
        String,
        String,
        String,
        String,
        String,
        f64,
        f64,
        String,
    ) = conn
        .query_row(
            "SELECT service_name, operation, signal, kind, direction, \
                    \"before\", \"after\", drill_down_sql \
             FROM sigiro_anomalies ORDER BY ts_shift DESC LIMIT 1",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            },
        )
        .expect("anomaly row");
    assert_eq!(service, "worker");
    assert_eq!(operation, "call upstream");
    assert_eq!(signal, "error_rate");
    assert_eq!(kind, "changepoint");
    assert_eq!(direction, "worsening");
    assert!(
        after > before,
        "worsening means mean after > before ({before} → {after})"
    );

    // The row's drill query passes the /v1/query firewall AND executes.
    sigiro::server::validate_read_only(&drill)
        .expect("anomaly drill_down_sql must pass the /v1/query guard");
    let drilled: i64 = conn
        .query_row(&format!("SELECT count(*) FROM ({drill}) t"), [], |r| {
            r.get(0)
        })
        .expect("drill query must execute");
    assert!(drilled > 0, "drill query must return the new-regime spans");

    // /investigate over a window containing the shift (~50 min ago) reads the table.
    let block = investigate_service(&conn, "worker", now_us - 3_600_000_000, now_us);
    let op = &block.operation_summaries[0];
    assert_eq!(op.operation, "call upstream");
    assert!(
        op.is_anomalous,
        "an in-window worsening regime shift must flag"
    );
    let reason = op.anomaly_reason.as_deref().expect("anomaly_reason");
    assert!(
        reason.contains("error rate shifted at"),
        "reason should name the shift, got: {reason}"
    );

    // The detected shift surfaces as a critical finding carrying the drill.
    let finding = block
        .findings
        .iter()
        .find(|f| f.kind == "anomalous_operation")
        .expect("anomalous operation should be a top-level finding");
    assert_eq!(finding.severity, "critical");
    assert!(finding.summary.contains("call upstream"));
    assert!(finding.drill_down_sql.is_some());

    // A window that ENDS BEFORE the shift (~50 min ago) must not flag: the
    // anomaly row exists but its ts_shift is outside [from, to]. This window
    // sits entirely in the healthy region (180..65 min ago).
    let early = investigate_service(
        &conn,
        "worker",
        now_us - 10_800_000_000,
        now_us - 3_900_000_000,
    );
    if let Some(op) = early.operation_summaries.first() {
        assert!(!op.is_anomalous, "shift outside the window must not flag");
    }
}

#[test]
fn anomaly_pass_is_idempotent() {
    // Repeated passes replace, never accumulate: same telemetry in, same
    // row count out.
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    seed_worsening_shift(&conn, now_us);

    let first = run_anomaly_pass(&conn);
    assert!(first >= 1);
    let second = run_anomaly_pass(&conn);
    assert_eq!(first, second, "re-running the pass must not change results");

    let count: i64 = conn
        .query_row("SELECT count(*) FROM sigiro_anomalies", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        count as usize, second,
        "table holds exactly one pass's rows — no duplicate accumulation"
    );
}

#[test]
fn anomaly_pass_fails_loud_but_is_isolated_by_the_caller() {
    // A broken schema makes detection itself error (fail loud)...
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    conn.execute_batch("DROP TABLE sigiro_spans").unwrap();
    let err = sigiro::anomaly::detect(&conn, "")
        .expect_err("detection must surface the failure, never swallow it");
    assert!(
        format!("{err:#}").contains("anomaly"),
        "error names the failing stage: {err:#}"
    );
    // ...and the inserter's detection task catches exactly this Err and logs
    // it (spawn_anomaly_detection; persist errors are likewise caught in
    // ProjectFlusher::persist_anomalies), so ingest/flush never see it.
}

#[test]
fn anomaly_chronic_stable_error_rate_is_not_anomalous() {
    // INTENDED semantics: a failure rate that is flat across the whole range
    // (30% for 74 straight minutes) has NO changepoint — it IS the baseline.
    // Chronic badness stays visible through error_pct, not the anomaly flag.
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    let min = 60_000_000_i64;
    for i in 1..=74_i64 {
        insert_spans(
            &conn,
            "worker",
            "call upstream",
            now_us - i * min,
            (10, 3),
            10_000,
            None,
        );
    }

    let written = run_anomaly_pass(&conn);
    assert_eq!(
        written, 0,
        "flat 30% error rate across the whole range: no changepoint, no row"
    );

    let block = investigate_service(&conn, "worker", now_us - 900_000_000, now_us);
    let op = &block.operation_summaries[0];
    assert!(!op.is_anomalous);
    assert!(op.anomaly_reason.is_none());
    assert!(
        !block
            .findings
            .iter()
            .any(|f| f.kind == "anomalous_operation"),
        "no anomaly finding for a regime that never shifted"
    );
    // The chronic badness itself is still on the record as evidence.
    assert!((op.error_pct - 30.0).abs() < 0.01);
}

#[test]
fn anomaly_improvement_shift_is_not_persisted() {
    // A DOWNWARD shift (50% errors drops to 0%) is a changepoint, but the
    // after-mean is BETTER — improvements never land in the table.
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    let min = 60_000_000_i64;
    for i in 16..=74_i64 {
        insert_spans(
            &conn,
            "worker",
            "call upstream",
            now_us - i * min,
            (10, 5),
            10_000,
            None,
        );
    }
    for i in 1..=12_i64 {
        insert_spans(
            &conn,
            "worker",
            "call upstream",
            now_us - i * min,
            (10, 0),
            10_000,
            None,
        );
    }

    let written = run_anomaly_pass(&conn);
    assert_eq!(written, 0, "getting BETTER is a shift but never an anomaly");

    let block = investigate_service(&conn, "worker", now_us - 900_000_000, now_us);
    let op = &block.operation_summaries[0];
    assert!(!op.is_anomalous);
}

#[test]
fn anomaly_latency_only_shift_is_out_of_scope_for_the_error_rate_slice() {
    // Zero errors throughout, p95 jumps 10ms → 300ms: the spans/error-rate
    // slice detects error-rate shifts ONLY — the latency signal lands in a
    // later slice (docs/plans/01-continuous-anomaly-subsystem.md P1), so no
    // row is persisted and nothing flags.
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    let min = 60_000_000_i64;
    for i in 16..=74_i64 {
        insert_spans(
            &conn,
            "api",
            "GET /slow",
            now_us - i * min,
            (10, 0),
            10_000,
            None,
        );
    }
    for i in 1..=12_i64 {
        insert_spans(
            &conn,
            "api",
            "GET /slow",
            now_us - i * min,
            (10, 0),
            300_000,
            None,
        );
    }

    let written = run_anomaly_pass(&conn);
    assert_eq!(written, 0, "error-rate series is flat zero — no row");

    let block = investigate_service(&conn, "api", now_us - 900_000_000, now_us);
    assert!(!block.operation_summaries[0].is_anomalous);
}

#[test]
fn anomaly_single_bucket_series_has_no_changepoint() {
    // All spans land in ONE bucket: BOCPD on a 1-point series emits no
    // changepoint BY CONSTRUCTION — no minimum-points gate needed, and no
    // row is persisted.
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    insert_spans(
        &conn,
        "api",
        "new-broken",
        now_us - 120_000_000,
        (25, 15),
        1_000,
        None,
    );

    let written = run_anomaly_pass(&conn);
    assert_eq!(written, 0, "a 1-bucket series has no regime to shift from");

    let block = investigate_service(&conn, "api", now_us - 900_000_000, now_us);
    let op = &block.operation_summaries[0];
    assert!(!op.is_anomalous);
    assert!(op.anomaly_reason.is_none());
}

#[test]
fn anomaly_findings_rank_by_shift_magnitude_without_dropping_any() {
    // Rank, don't filter: two operations shift in the same window — one big
    // (0% → ~60%), one small (0% → ~10%). BOTH surface (threshold-free), and
    // the biggest shift leads the findings so the agent reads it first.
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    // Same shape as seed_worsening_shift: ~30 healthy 5-min buckets, then
    // ~10 worsening ones — per operation, with different shift sizes.
    let bucket = 5 * 60_000_000_i64;
    for (span_name, errors_after) in [("op-small", 1), ("op-big", 6)] {
        for i in 11..=40_i64 {
            insert_spans(
                &conn,
                "worker",
                span_name,
                now_us - i * bucket,
                (10, 0),
                10_000,
                None,
            );
        }
        for i in 1..=10_i64 {
            insert_spans(
                &conn,
                "worker",
                span_name,
                now_us - i * bucket,
                (10, errors_after),
                10_000,
                None,
            );
        }
    }

    let written = run_anomaly_pass(&conn);
    assert!(written >= 2, "both shifts must persist, got {written}");

    // /v1/anomalies ordering contract, straight off the table: biggest
    // |after - before| first (the handler's ORDER BY).
    let ordered: Vec<String> = conn
        .prepare(
            "SELECT operation FROM sigiro_anomalies \
             ORDER BY abs(\"after\" - \"before\") DESC, ts_shift DESC",
        )
        .expect("prepare")
        .query_map([], |r| r.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    assert_eq!(
        ordered.first().map(String::as_str),
        Some("op-big"),
        "the biggest shift must lead, got {ordered:?}"
    );
    assert!(
        ordered.iter().any(|op| op == "op-small"),
        "the small shift is ranked lower but NEVER dropped, got {ordered:?}"
    );

    // /investigate: both ops flag, and the anomalous_operation findings are
    // ordered biggest shift first.
    let block = investigate_service(&conn, "worker", now_us - 3_600_000_000, now_us);
    for op in &block.operation_summaries {
        assert!(op.is_anomalous, "operation '{}' must flag", op.operation);
    }
    let anomaly_findings: Vec<&str> = block
        .findings
        .iter()
        .filter(|f| f.kind == "anomalous_operation")
        .map(|f| f.summary.as_str())
        .collect();
    assert_eq!(
        anomaly_findings.len(),
        2,
        "threshold-free: every detected shift surfaces as a finding"
    );
    assert!(
        anomaly_findings[0].contains("op-big"),
        "biggest shift leads the findings, got {anomaly_findings:?}"
    );
    assert!(
        anomaly_findings[1].contains("op-small"),
        "smaller shift follows, got {anomaly_findings:?}"
    );
}

// ---------------------------------------------------------------------------
// Operation identity (#3): bare HTTP-method span names derive from attributes
// ---------------------------------------------------------------------------

#[test]
fn generic_http_span_names_derive_operation_from_attributes() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    // All spans are named "POST" (OTel HTTP-client convention) — identity is
    // in span_attributes. Two hosts, very different health.
    insert_spans(
        &conn,
        "worker",
        "POST",
        now_us,
        (8, 8),
        1_000,
        Some(r#"{"server.address":"api.nvidia.com"}"#),
    );
    insert_spans(
        &conn,
        "worker",
        "POST",
        now_us + 1_000,
        (12, 0),
        1_000,
        Some(r#"{"server.address":"api.stripe.com"}"#),
    );
    // http.route wins over server.address when both are present.
    insert_spans(
        &conn,
        "worker",
        "GET",
        now_us + 2_000,
        (3, 0),
        1_000,
        Some(r#"{"http.route":"/orders/{id}","server.address":"internal"}"#),
    );
    // Bare method with no attributes stays as-is.
    insert_spans(&conn, "worker", "PUT", now_us + 3_000, (2, 0), 1_000, None);

    let block = investigate_service(&conn, "worker", now_us - 60_000_000, now_us + 60_000_000);
    let ops: Vec<&str> = block
        .operation_summaries
        .iter()
        .map(|o| o.operation.as_str())
        .collect();
    assert!(
        ops.contains(&"POST api.nvidia.com") && ops.contains(&"POST api.stripe.com"),
        "one giant POST bucket must split by server.address, got: {ops:?}"
    );
    assert!(
        ops.contains(&"GET /orders/{id}"),
        "http.route preferred: {ops:?}"
    );
    assert!(
        ops.contains(&"PUT"),
        "attribute-less method stays bare: {ops:?}"
    );

    let nvidia = block
        .operation_summaries
        .iter()
        .find(|o| o.operation == "POST api.nvidia.com")
        .unwrap();
    assert_eq!(nvidia.span_name, "POST");
    assert_eq!(nvidia.count, 8);
    assert!((nvidia.error_pct - 100.0).abs() < 0.01);

    // CRITICAL: the drill query must reconcile exactly with the operation's
    // count — filtering `span_name = 'POST'` would return 20 rows, not 8.
    let drill = nvidia.drill_down_sql.as_deref().expect("drill sql");
    let drilled: i64 = conn
        .query_row(&format!("SELECT count(*) FROM ({drill}) t"), [], |r| {
            r.get(0)
        })
        .expect("drill query must execute");
    assert_eq!(
        drilled, nvidia.count,
        "drilled rows must match the operation's count exactly"
    );
    // And it must pass the /v1/query firewall verbatim.
    sigiro::server::validate_read_only(drill)
        .expect("operation drill_down_sql must pass the /v1/query guard");
}

// ---------------------------------------------------------------------------
// Deploy events (#7): steady versions are not deploys
// ---------------------------------------------------------------------------

#[test]
fn deploy_events_suppressed_for_steady_version() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    let baseline_ts = now_us - 600 * 1_000_000;
    conn.execute_batch(&format!(
        "INSERT INTO sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id,
            parent_span_id, span_name, span_kind, status_code, service_name, resource_attributes)
         VALUES
            -- v1 running BEFORE the window (baseline) and inside it: steady state.
            (make_timestamp({baseline_ts}), {baseline_ts}+1000, 1000, 't0', 's0', '', 'op', 2, 1, 'api',
             '{{\"service.version\":\"v1\"}}'),
            (make_timestamp({now_us}), {now_us}+1000, 1000, 't1', 's1', '', 'op', 2, 1, 'api',
             '{{\"service.version\":\"v1\"}}'),
            -- v2 appears ONLY inside the window: a real deploy.
            (make_timestamp({now_us} + 1000), {now_us}+2000, 1000, 't2', 's2', '', 'op', 2, 1, 'api',
             '{{\"service.version\":\"v2\"}}')"
    ))
    .expect("insert");
    let events =
        investigate_service(&conn, "api", now_us - 300_000_000, now_us + 60_000_000).events;
    let deploys: Vec<_> = events.iter().filter(|e| e.event_type == "deploy").collect();
    assert_eq!(
        deploys.len(),
        1,
        "steady v1 must NOT fabricate a deploy at window start; only v2 is new: {deploys:?}"
    );
    assert!(deploys[0].description.contains("v2"));
}

// ---------------------------------------------------------------------------
// status_message + errors-without-logs (#5) and findings (#4)
// ---------------------------------------------------------------------------

#[test]
fn status_message_and_errors_without_logs_finding() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    conn.execute_batch(&format!(
        "INSERT INTO sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id,
            parent_span_id, span_name, span_kind, status_code, service_name, status_message)
         VALUES
            (make_timestamp({now_us}), {now_us}+1000, 1000, 't1', 's1', '', 'POST', 3, 2, 'worker',
             '429 Too Many Requests')"
    ))
    .expect("insert");

    let block = investigate_service(&conn, "worker", now_us - 60_000_000, now_us + 60_000_000);

    // status_message is often the ONLY diagnostic for log-less failures.
    assert_eq!(
        block.spans.sample[0].status_message.as_deref(),
        Some("429 Too Many Requests")
    );

    // Error spans + zero ERROR logs → a warning finding with a drill.
    let f = block
        .findings
        .iter()
        .find(|f| f.kind == "errors_without_logs")
        .expect("errors_without_logs finding");
    assert_eq!(f.severity, "warning");
    assert!(
        f.drill_down_sql
            .as_deref()
            .unwrap()
            .contains("status_code = 2")
    );

    // Spans but no logs at all → coverage_gap finding too.
    assert!(block.findings.iter().any(|f| f.kind == "coverage_gap"));
    // No k8s resource attributes → info finding.
    assert!(
        block
            .findings
            .iter()
            .any(|f| f.kind == "missing_resource_attributes")
    );
    // Ranked: no info finding may appear before a warning.
    let sev_rank = |s: &str| match s {
        "critical" => 0,
        "warning" => 1,
        _ => 2,
    };
    let ranks: Vec<_> = block
        .findings
        .iter()
        .map(|f| sev_rank(&f.severity))
        .collect();
    let mut sorted = ranks.clone();
    sorted.sort_unstable();
    assert_eq!(ranks, sorted, "findings must be sorted by severity");
}

#[test]
fn errors_with_error_logs_do_not_emit_inconsistency_finding() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    conn.execute_batch(&format!(
        "INSERT INTO sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id,
            parent_span_id, span_name, span_kind, status_code, service_name)
         VALUES (make_timestamp({now_us}), {now_us}+1000, 1000, 't1', 's1', '', 'op', 2, 2, 'api');
         INSERT INTO sigiro_logs (timestamp, observed_timestamp, service_name, severity_number,
            severity_text, body)
         VALUES (make_timestamp({now_us}), {now_us}, 'api', 17, 'ERROR', 'it broke')"
    ))
    .expect("insert");
    let block = investigate_service(&conn, "api", now_us - 60_000_000, now_us + 60_000_000);
    assert!(
        !block
            .findings
            .iter()
            .any(|f| f.kind == "errors_without_logs"),
        "errors WITH error logs are consistent — no finding"
    );
}

// ---------------------------------------------------------------------------
// Partial results (#8): all-fail is still loud
// ---------------------------------------------------------------------------

#[test]
fn investigate_fails_loud_when_all_services_fail() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    // Break the schema out from under the query — every service now fails.
    conn.execute_batch("DROP TABLE sigiro_profiles").unwrap();
    let req = InvestigateRequest {
        service: Some("svc".to_string()),
        services: None,
        from_ts: 0,
        to_ts: 1,
    };
    let err = investigate(&conn, &req, "")
        .err()
        .expect("when EVERY service fails there is nothing partial to return — fail loud");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("svc"),
        "error should name the failing service: {msg}"
    );
}
