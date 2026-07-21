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
// Anomaly rules (#2): absolute floor, latency-vs-baseline, no-baseline cases
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

#[test]
fn anomaly_absolute_rule_flags_chronic_failures() {
    // The dogfooded gap: a 24%+ error rate that is ALSO 24% in the baseline
    // used to yield is_anomalous=false (relative rule only). The absolute
    // rule must flag it, and say it is chronic.
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    let baseline_ts = now_us - 600 * 1_000_000; // inside the 4x baseline window
    // Baseline: equally bad (25% errors).
    insert_spans(
        &conn,
        "worker",
        "call upstream",
        baseline_ts,
        (40, 10),
        50_000,
        None,
    );
    // Current: chronic 25% errors, count >= 20.
    insert_spans(
        &conn,
        "worker",
        "call upstream",
        now_us,
        (40, 10),
        50_000,
        None,
    );

    let block = investigate_service(&conn, "worker", now_us - 300_000_000, now_us + 60_000_000);
    let op = &block.operation_summaries[0];
    assert_eq!(op.operation, "call upstream");
    assert_eq!(
        op.is_anomalous,
        Some(true),
        "chronic 25% error rate must flag via the absolute rule even with an equally bad baseline"
    );
    let reason = op.anomaly_reason.as_deref().expect("anomaly_reason");
    assert!(
        reason.contains("absolute threshold"),
        "reason should name the absolute rule, got: {reason}"
    );
    assert!(
        reason.contains("chronic"),
        "reason should say the failure is chronic when baseline is also elevated, got: {reason}"
    );

    // The anomaly must surface as a top-level finding carrying the drill.
    let finding = block
        .findings
        .iter()
        .find(|f| f.kind == "anomalous_operation")
        .expect("anomalous operation should be a top-level finding");
    assert_eq!(finding.severity, "critical");
    assert!(finding.summary.contains("call upstream"));
    assert!(finding.drill_down_sql.is_some());
}

#[test]
fn anomaly_latency_rule_uses_baseline_p95() {
    // bl_p95 used to be computed and then IGNORED by the anomaly logic.
    // A 30x latency regression with zero errors must flag.
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    let baseline_ts = now_us - 600 * 1_000_000;
    insert_spans(
        &conn,
        "api",
        "GET /slow",
        baseline_ts,
        (10, 0),
        10_000,
        None,
    ); // 10ms
    insert_spans(&conn, "api", "GET /slow", now_us, (10, 0), 300_000, None); // 300ms

    let block = investigate_service(&conn, "api", now_us - 300_000_000, now_us + 60_000_000);
    let op = &block.operation_summaries[0];
    assert_eq!(op.is_anomalous, Some(true), "latency regression must flag");
    let reason = op.anomaly_reason.as_deref().expect("anomaly_reason");
    assert!(
        reason.contains("p95 latency"),
        "reason should name the latency rule, got: {reason}"
    );
}

#[test]
fn anomaly_no_baseline_cases() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    // Brand-new failing op (no baseline): absolute rule must still fire.
    insert_spans(&conn, "api", "new-broken", now_us, (25, 15), 1_000, None);
    // Brand-new healthy op (no baseline): can't judge yet → None.
    insert_spans(&conn, "api", "new-healthy", now_us, (25, 0), 1_000, None);

    let block = investigate_service(&conn, "api", now_us - 300_000_000, now_us + 60_000_000);
    let broken = block
        .operation_summaries
        .iter()
        .find(|o| o.operation == "new-broken")
        .unwrap();
    assert_eq!(
        broken.is_anomalous,
        Some(true),
        "no-baseline op tripping the absolute rule must flag, not stay NULL"
    );
    let healthy = block
        .operation_summaries
        .iter()
        .find(|o| o.operation == "new-healthy")
        .unwrap();
    assert_eq!(
        healthy.is_anomalous, None,
        "healthy op with no baseline is unjudgeable → None"
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
