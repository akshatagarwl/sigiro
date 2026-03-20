use sigiro::investigate::{
    InvestigateMultiRequest, InvestigateRequest, SpanRow, adaptive_config, build_span_trees,
    compute_coverage, investigate, investigate_multi, query_derived_events, query_logs,
    query_service_context, us_to_iso,
};
use std::sync::{Arc, Mutex};

fn make_span(trace_id: &str, span_id: &str, parent: &str, status: i32) -> SpanRow {
    SpanRow {
        trace_id: trace_id.to_string(),
        span_id: span_id.to_string(),
        parent_span_id: parent.to_string(),
        span_name: "test".to_string(),
        duration_ns: 1000,
        status_code: status,
        service_name: "svc".to_string(),
    }
}

#[test]
fn build_span_trees_groups_by_parent() {
    let spans = vec![
        make_span("t1", "s1", "", 0),
        make_span("t1", "s2", "s1", 0),
        make_span("t1", "s3", "s1", 0),
        make_span("t1", "s4", "s2", 0),
    ];
    let trees = build_span_trees(&spans);
    assert_eq!(trees.get("s1").unwrap().len(), 2);
    assert_eq!(trees.get("s2").unwrap().len(), 1);
    assert!(!trees.contains_key(""));
}

#[test]
fn build_span_trees_empty() {
    let trees = build_span_trees(&[]);
    assert!(trees.is_empty());
}

#[test]
fn build_span_trees_root_only() {
    let spans = vec![make_span("t1", "s1", "", 0)];
    let trees = build_span_trees(&spans);
    assert!(trees.is_empty());
}

#[test]
fn us_to_iso_valid() {
    let iso = us_to_iso(1_700_000_000_000_000);
    assert!(iso.contains("2023-11"));
}

#[test]
fn us_to_iso_zero() {
    let iso = us_to_iso(0);
    assert!(iso.contains("1970"));
}

#[test]
fn investigate_with_in_memory_db() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn).expect("create tables");
    let shared = Arc::new(Mutex::new(conn));
    let req = InvestigateRequest {
        service: "nonexistent".to_string(),
        from_ts: None,
        to_ts: None,
    };
    let block = investigate(&shared, &req).expect("investigate should succeed on empty db");
    assert_eq!(block.service, "nonexistent");
    assert!(block.operation_summaries.is_empty());
    assert!(block.metrics.is_empty());
    assert!(block.spans.is_empty());
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
        block.coverage.gaps.len() >= 3,
        "should have gaps for all signal types"
    );
    assert!(block.coverage.gaps.iter().any(|g| g.contains("no spans")));
    assert!(block.coverage.gaps.iter().any(|g| g.contains("no logs")));
    assert!(block.coverage.gaps.iter().any(|g| g.contains("no metrics")));
}

#[test]
fn investigate_with_real_span_data() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn).expect("create tables");

    let now_us = chrono::Utc::now().timestamp_micros();
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
        service: "checkout".to_string(),
        from_ts: Some(now_us - 60_000_000),
        to_ts: Some(now_us + 60_000_000),
    };
    let block = investigate(&shared, &req).expect("investigate");

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

    assert_eq!(block.spans.len(), 5);
    assert_eq!(block.spans[0].status_code, 2);
    assert_eq!(block.truncation.spans_total, 5);
    assert_eq!(block.truncation.spans_returned, 5);
    assert_eq!(block.truncation.error_traces_total, 2);

    assert!(block.coverage.has_spans);
    assert!(!block.coverage.has_logs);
    assert!(!block.coverage.has_metrics);
    assert_eq!(block.coverage.span_count, 5);
    assert!(block.coverage.gaps.iter().any(|g| g.contains("no logs")));
    assert!(
        block
            .coverage
            .gaps
            .iter()
            .any(|g| g.contains("spans exist but no logs"))
    );
    assert!(!block.coverage.gaps.iter().any(|g| g.contains("no spans")));
}

#[test]
fn coverage_no_data() {
    let c = compute_coverage(0, 0, 0);
    assert!(!c.has_spans);
    assert!(!c.has_logs);
    assert!(!c.has_metrics);
    assert!(c.gaps.len() >= 3);
}

#[test]
fn coverage_spans_only() {
    let c = compute_coverage(10, 0, 0);
    assert!(c.has_spans);
    assert!(!c.has_logs);
    assert!(!c.has_metrics);
    assert!(c.gaps.iter().any(|g| g.contains("spans exist but no logs")));
    assert!(!c.gaps.iter().any(|g| g.contains("no spans")));
}

#[test]
fn coverage_all_signals() {
    let c = compute_coverage(10, 5, 3);
    assert!(c.has_spans);
    assert!(c.has_logs);
    assert!(c.has_metrics);
    assert!(c.gaps.is_empty(), "no gaps when all signals present");
}

#[test]
fn service_context_empty_db() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn).expect("create tables");
    let now_us = chrono::Utc::now().timestamp_micros();
    let ctx = query_service_context(&conn, "nonexistent", now_us - 60_000_000, now_us).unwrap();
    assert_eq!(ctx.service_name, "nonexistent");
    assert_eq!(ctx.pod_count, 0);
    assert!(ctx.dependencies.is_empty());
    assert!(ctx.dependents.is_empty());
    assert!(ctx.namespace.is_none());
}

#[test]
fn service_context_with_k8s_attrs() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn).expect("create tables");
    let now_us = chrono::Utc::now().timestamp_micros();
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
        query_service_context(&conn, "checkout", now_us - 60_000_000, now_us + 60_000_000).unwrap();
    assert_eq!(ctx.namespace, Some("prod".to_string()));
    assert_eq!(ctx.deployment, Some("checkout-v2".to_string()));
    assert_eq!(ctx.pod_count, 2);
}

#[test]
fn service_context_dependencies() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn).expect("create tables");
    let now_us = chrono::Utc::now().timestamp_micros();
    conn.execute_batch(&format!(
        "INSERT INTO sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id,
            parent_span_id, span_name, span_kind, status_code, service_name)
         VALUES
            (make_timestamp({now_us}), {now_us}+1000, 1000, 'trace-x', 'sx1', '', 'POST /pay', 3, 1, 'checkout'),
            (make_timestamp({now_us}), {now_us}+1000, 1000, 'trace-x', 'sx2', 'sx1', 'process', 2, 1, 'payments')"
    ))
    .expect("insert");
    let ctx =
        query_service_context(&conn, "checkout", now_us - 60_000_000, now_us + 60_000_000).unwrap();
    assert!(
        ctx.dependencies.contains(&"payments".to_string()),
        "checkout should depend on payments"
    );
    let ctx2 =
        query_service_context(&conn, "payments", now_us - 60_000_000, now_us + 60_000_000).unwrap();
    assert!(
        ctx2.dependents.contains(&"checkout".to_string()),
        "payments should have checkout as dependent"
    );
}

#[test]
fn derived_events_deploy_detection() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn).expect("create tables");
    let now_us = chrono::Utc::now().timestamp_micros();
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
        query_derived_events(&conn, "api", now_us - 120_000_000, now_us + 60_000_000).unwrap();
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
    sigiro::schema::create_tables(&conn).expect("create tables");
    let now_us = chrono::Utc::now().timestamp_micros();
    conn.execute_batch(&format!(
        "INSERT INTO sigiro_logs (timestamp, observed_timestamp, service_name, severity_number,
            severity_text, body)
         VALUES
            (make_timestamp({now_us}), {now_us}, 'api', 17, 'ERROR',
             'Container killed due to OOMKilled')"
    ))
    .expect("insert");
    let events =
        query_derived_events(&conn, "api", now_us - 60_000_000, now_us + 60_000_000).unwrap();
    let k8s: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "k8s_event")
        .collect();
    assert_eq!(k8s.len(), 1);
    assert!(k8s[0].description.contains("OOMKilled"));
}

#[test]
fn log_template_dedup_ips() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn).expect("create tables");
    let now_us = chrono::Utc::now().timestamp_micros();
    conn.execute_batch(&format!(
        "INSERT INTO sigiro_logs (timestamp, observed_timestamp, service_name, severity_number,
            severity_text, body)
         VALUES
            (make_timestamp({now_us}), {now_us}, 'api', 13, 'WARN', 'Connection timeout for 192.168.1.1'),
            (make_timestamp({now_us}), {now_us}, 'api', 13, 'WARN', 'Connection timeout for 10.0.0.5'),
            (make_timestamp({now_us}), {now_us}, 'api', 13, 'WARN', 'Connection timeout for 172.16.0.99')"
    ))
    .expect("insert");
    let logs = query_logs(&conn, "api", now_us - 60_000_000, now_us + 60_000_000, 30).unwrap();
    assert_eq!(
        logs.len(),
        1,
        "3 logs with different IPs should collapse to 1 template"
    );
    assert_eq!(logs[0].count, 3);
    assert!(logs[0].template.contains("{ip}"));
    assert!(
        logs[0].body.contains("192.168")
            || logs[0].body.contains("10.0.0")
            || logs[0].body.contains("172.16")
    );
}

#[test]
fn log_template_dedup_uuids() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn).expect("create tables");
    let now_us = chrono::Utc::now().timestamp_micros();
    conn.execute_batch(&format!(
        "INSERT INTO sigiro_logs (timestamp, observed_timestamp, service_name, severity_number,
            severity_text, body)
         VALUES
            (make_timestamp({now_us}), {now_us}, 'api', 17, 'ERROR', 'Failed to process order 550e8400-e29b-41d4-a716-446655440000'),
            (make_timestamp({now_us}), {now_us}, 'api', 17, 'ERROR', 'Failed to process order a1b2c3d4-e5f6-7890-abcd-ef1234567890')"
    ))
    .expect("insert");
    let logs = query_logs(&conn, "api", now_us - 60_000_000, now_us + 60_000_000, 30).unwrap();
    assert_eq!(logs.len(), 1, "2 logs with different UUIDs should collapse");
    assert!(logs[0].template.contains("{uuid}"));
    assert_eq!(logs[0].count, 2);
}

#[test]
fn adaptive_config_short_window() {
    let now = 1_000_000_000_000_i64;
    let config = adaptive_config(now - 5 * 60 * 1_000_000, now);
    assert_eq!(config.bucket_interval, "1 minute");
    assert_eq!(config.span_limit, 50);
    assert_eq!(config.log_limit, 30);
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
    assert_eq!(config.log_limit, 20);
}

#[test]
fn investigate_large_window_no_spans() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn).expect("create tables");
    let now_us = chrono::Utc::now().timestamp_micros();
    conn.execute_batch(&format!(
        "INSERT INTO sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id,
            parent_span_id, span_name, span_kind, status_code, service_name)
         VALUES
            (make_timestamp({now_us}), {now_us}+1000, 1000, 't1', 's1', '', 'op', 2, 1, 'svc')"
    ))
    .expect("insert");
    let shared = Arc::new(Mutex::new(conn));
    let req = InvestigateRequest {
        service: "svc".to_string(),
        from_ts: Some(now_us - 3 * 3600 * 1_000_000),
        to_ts: Some(now_us + 60_000_000),
    };
    let block = investigate(&shared, &req).unwrap();
    assert!(
        block.spans.is_empty(),
        ">2h window should omit flat spans (operation_summaries suffice)"
    );
    assert!(!block.operation_summaries.is_empty());
}

#[test]
fn investigate_multi_basic() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn).expect("create tables");
    let now_us = chrono::Utc::now().timestamp_micros();
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
    let req = InvestigateMultiRequest {
        services: vec!["svc-a".into(), "svc-b".into(), "svc-c".into()],
        from_ts: Some(now_us - 60_000_000),
        to_ts: Some(now_us + 60_000_000),
    };
    let blocks = investigate_multi(&shared, &req).unwrap();
    assert_eq!(blocks.len(), 3);
    assert!(blocks.contains_key("svc-a"));
    assert!(blocks.contains_key("svc-b"));
    assert!(blocks.contains_key("svc-c"));
}

#[test]
fn investigate_multi_cap_enforced() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn).expect("create tables");
    let shared = Arc::new(Mutex::new(conn));
    let services: Vec<String> = (0..11).map(|i| format!("svc-{i}")).collect();
    let req = InvestigateMultiRequest {
        services,
        from_ts: None,
        to_ts: None,
    };
    let result = investigate_multi(&shared, &req);
    assert!(result.is_err(), "should reject >10 services");
}

#[test]
fn investigate_multi_empty_rejected() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn).expect("create tables");
    let shared = Arc::new(Mutex::new(conn));
    let req = InvestigateMultiRequest {
        services: vec![],
        from_ts: None,
        to_ts: None,
    };
    assert!(investigate_multi(&shared, &req).is_err());
}

#[test]
fn derived_events_no_events() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn).expect("create tables");
    let now_us = chrono::Utc::now().timestamp_micros();
    let events = query_derived_events(&conn, "api", now_us - 60_000_000, now_us).unwrap();
    assert!(events.is_empty());
}

#[test]
fn operation_summary_baselines() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
    sigiro::schema::create_tables(&conn).expect("create tables");

    let now_us = chrono::Utc::now().timestamp_micros();
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
        service: "checkout".to_string(),
        from_ts: Some(now_us - 5 * 60 * 1_000_000),
        to_ts: Some(now_us + 60_000_000),
    };
    let block = investigate(&shared, &req).expect("investigate");

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
