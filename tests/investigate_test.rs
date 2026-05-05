use sigiro::investigate::{
    InvestigateRequest, QueryCtx, Span, adaptive_config, compute_coverage,
    investigate, query_derived_events, query_service_context, us_to_iso,
};
use sigiro::types::StatusCode;
use std::sync::{Arc, Mutex};

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
        let (template, id) = drain.classify(body);
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

fn make_span(trace_id: &str, span_id: &str, parent: &str, status: StatusCode) -> Span {
    Span {
        trace_id: trace_id.to_string(),
        span_id: span_id.to_string(),
        parent_span_id: parent.to_string(),
        span_name: "test".to_string(),
        duration_ns: 1000,
        status_code: status,
        service_name: "svc".to_string(),
        span_kind: None,
        span_attributes: None,
        resource_attributes: None,
        start_timestamp: 0,
        end_timestamp: 0,
        events_json: None,
        links_json: None,
        instrumentation_library_name: None,
        instrumentation_library_version: None,
        trace_state: None,
        scope_span: None,
    }
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
    let blocks = investigate(&shared, &req, "").expect("investigate should succeed on empty db");
    let block = &blocks["nonexistent"];
    assert_eq!(block.service, "nonexistent");
    assert!(block.operation_summaries.is_empty());
    assert!(block.metrics.is_empty());
    assert!(block.traces.is_empty());
    assert!(block.logs.is_empty());
    assert!(block.linked_traces.is_empty());
    assert_eq!(block.truncation.spans_total, 0);
    assert_eq!(block.truncation.logs_total, 0);
    assert_eq!(block.truncation.error_traces_total, 0);

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
    assert!(block.coverage.signals.iter().any(|s| s.reason.contains("no spans")));
    assert!(block.coverage.signals.iter().any(|s| s.reason.contains("no logs")));
    assert!(block.coverage.signals.iter().any(|s| s.reason.contains("no metrics")));
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
    let blocks = investigate(&shared, &req, "").expect("investigate");
    let block = &blocks["checkout"];

    assert_eq!(block.operation_summaries.len(), 2);
    let checkout_op = &block.operation_summaries[0];
    assert_eq!(checkout_op.span_name, "POST /checkout");
    assert_eq!(checkout_op.count, 3);
    assert_eq!(checkout_op.error_count, 2);
    assert!((checkout_op.error_pct - 66.66).abs() < 1.0);
    assert!(checkout_op.exemplar_error_trace_id.is_some());
    assert!(checkout_op.baseline_error_pct.is_none());
    assert!(checkout_op.baseline_p95_ns.is_none());
    assert!(checkout_op.baseline_count.is_none());

    let health_op = &block.operation_summaries[1];
    assert_eq!(health_op.span_name, "GET /health");
    assert_eq!(health_op.count, 2);
    assert_eq!(health_op.error_count, 0);
    assert!(health_op.exemplar_error_trace_id.is_none());

    assert_eq!(block.traces.len(), 5);
    assert_eq!(block.traces[0].root_span.status_code, StatusCode::Error);
    assert_eq!(block.truncation.spans_total, 5);
    assert_eq!(block.truncation.spans_returned, 5);
    assert_eq!(block.truncation.error_traces_total, 2);

    assert!(block.coverage.has_spans);
    assert!(!block.coverage.has_logs);
    assert!(!block.coverage.has_metrics);
    assert_eq!(block.coverage.span_count, 5);
    assert!(block.coverage.signals.iter().any(|s| s.reason.contains("no logs")));
    assert!(
        block
            .coverage.signals
            .iter()
            .any(|s| s.reason.contains("spans exist but no logs"))
    );
    assert!(!block.coverage.signals.iter().any(|s| s.reason.contains("no spans")));
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
    assert!(c.signals.iter().any(|s| s.reason.contains("spans exist but no logs")));
    assert!(!c.signals.iter().any(|s| s.reason.contains("no spans")));
}

#[test]
fn coverage_all_signals() {
    let c = compute_coverage(10, 5, 3, 1);
    assert!(c.has_spans);
    assert!(c.has_logs);
    assert!(c.has_metrics);
    assert!(c.has_profiles);
    assert!(!c.signals.iter().any(|s| !s.available && s.severity == "high"), "no critical gaps when all signals present");
}

#[test]
fn service_context_empty_db() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    let qctx = QueryCtx {
        conn: &conn,
        service: "nonexistent",
        from_ts: now_us - 60_000_000,
        to_ts: now_us,
        prefix: "",
    };
    let ctx = query_service_context(&qctx).unwrap();
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
    let qctx = QueryCtx {
        conn: &conn,
        service: "checkout",
        from_ts: now_us - 60_000_000,
        to_ts: now_us + 60_000_000,
        prefix: "",
    };
    let ctx = query_service_context(&qctx).unwrap();
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
    let qctx = QueryCtx {
        conn: &conn,
        service: "checkout",
        from_ts: now_us - 60_000_000,
        to_ts: now_us + 60_000_000,
        prefix: "",
    };
    let ctx = query_service_context(&qctx).unwrap();
    assert!(
        ctx.dependencies.iter().any(|d| d.service_name == "payments"),
        "checkout should depend on payments"
    );
    let qctx2 = QueryCtx {
        conn: &conn,
        service: "payments",
        from_ts: now_us - 60_000_000,
        to_ts: now_us + 60_000_000,
        prefix: "",
    };
    let ctx2 = query_service_context(&qctx2).unwrap();
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
    let qctx = QueryCtx {
        conn: &conn,
        service: "api",
        from_ts: now_us - 120_000_000,
        to_ts: now_us + 60_000_000,
        prefix: "",
    };
    let events = query_derived_events(&qctx).unwrap();
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
    let qctx = QueryCtx {
        conn: &conn,
        service: "api",
        from_ts: now_us - 60_000_000,
        to_ts: now_us + 60_000_000,
        prefix: "",
    };
    let events = query_derived_events(&qctx).unwrap();
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

    let conn = shared.lock().unwrap();
    let qctx = QueryCtx {
        conn: &conn,
        service: "api",
        from_ts: now_us - 60_000_000,
        to_ts: now_us + 60_000_000,
        prefix: "",
    };
    let logs = sigiro::query_raw_logs(&qctx, 30).unwrap();
    // With the new API, we get raw logs not aggregated templates
    // So we get 3 logs back, not 1
    assert_eq!(logs.len(), 3, "3 raw logs returned");
    assert!(!logs.is_empty());
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

    let conn = shared.lock().unwrap();
    let qctx = QueryCtx {
        conn: &conn,
        service: "api",
        from_ts: now_us - 60_000_000,
        to_ts: now_us + 60_000_000,
        prefix: "",
    };
    let logs = sigiro::query_raw_logs(&qctx, 30).unwrap();
    // With the new API, we get raw logs not aggregated templates
    // So we get 2 logs back, not 1
    assert_eq!(logs.len(), 2, "2 raw logs returned");
    assert!(!logs.is_empty());
}

#[test]
fn adaptive_config_short_window() {
    let now = 1_000_000_000_000_i64;
    let config = adaptive_config(now - 5 * 60 * 1_000_000, now);
    assert_eq!(config.bucket_interval, "1 minute");
    assert_eq!(config.span_limit, 50);
    assert_eq!(config.log_limit, 50);
}

#[test]
fn adaptive_config_medium_window() {
    let now = 1_000_000_000_000_i64;
    let config = adaptive_config(now - 30 * 60 * 1_000_000, now);
    assert_eq!(config.bucket_interval, "5 minutes");
    assert_eq!(config.span_limit, 20);
}

#[test]
fn adaptive_config_large_window() {
    let now = 1_000_000_000_000_i64;
    let config = adaptive_config(now - 3 * 3600 * 1_000_000, now);
    assert_eq!(config.bucket_interval, "1 hour");
    assert_eq!(config.span_limit, 0);
    assert_eq!(config.log_limit, 30);
}

#[test]
fn investigate_large_window_no_spans() {
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
    let blocks = investigate(&shared, &req, "").unwrap();
    let block = &blocks["svc"];
    assert!(
        block.traces.is_empty(),
        ">2h window should omit flat spans (operation_summaries suffice)"
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
    let blocks = investigate(&shared, &req, "").unwrap();
    assert_eq!(blocks.len(), 3);
    assert!(blocks.contains_key("svc-a"));
    assert!(blocks.contains_key("svc-b"));
    assert!(blocks.contains_key("svc-c"));
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
    let result = investigate(&shared, &req, "");
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
    assert!(investigate(&shared, &req, "").is_err());
}

#[test]
fn derived_events_no_events() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn, "").expect("create tables");
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };
    let qctx = QueryCtx {
        conn: &conn,
        service: "api",
        from_ts: now_us - 60_000_000,
        to_ts: now_us,
        prefix: "",
    };
    let events = query_derived_events(&qctx).unwrap();
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
    let blocks = investigate(&shared, &req, "").expect("investigate");
    let block = &blocks["svc"];

    // Coverage: all 3 signal types present, no gaps
    assert!(block.coverage.has_spans);
    assert!(block.coverage.has_logs);
    assert!(block.coverage.has_metrics);
    assert!(
        !block.coverage.signals.iter().any(|s| !s.available && s.severity == "high"),
        "no critical gaps when all signals present"
    );

    // Truncation present with correct counts
    assert_eq!(block.truncation.spans_total, 1);

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
    let blocks = investigate(&shared, &req, "").expect("investigate");
    let block = &blocks["checkout"];

    let op = &block.operation_summaries[0];
    assert_eq!(op.span_name, "POST /checkout");
    assert_eq!(op.count, 2);
    assert_eq!(op.error_count, 2);
    assert!((op.error_pct - 100.0).abs() < 0.01);
    assert!(op.baseline_error_pct.is_some());
    assert!((op.baseline_error_pct.unwrap() - 0.0).abs() < 0.01);
    assert!(op.baseline_count.is_some());
    assert_eq!(op.baseline_count.unwrap(), 2);
    assert!(op.baseline_p95_ns.is_some());
    assert!(op.baseline_p95_ns.unwrap() < 100_000_000.0);
}
