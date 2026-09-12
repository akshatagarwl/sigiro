//! Deterministic real DuckLake query-shape reproduction for memory-pressure work.
//!
//! This deliberately uses the same `ProjectPool` and HTTP router as the API,
//! rather than an in-memory DuckDB connection or a mock query executor. The
//! fixture is bounded by default for developer use; set
//! `SIGIRO_QUERY_MEMORY_ROWS` to a larger deterministic row count when a stress
//! run is needed. The default run is not an OOM reproduction.

#![allow(clippy::disallowed_methods)]

use axum::http::StatusCode;
use axum_test::TestServer;
use camino::Utf8Path;
use duckdb::Connection;
use serde_json::Value;
use sigiro::admin;
use sigiro::inserter::MsgRouter;
use sigiro::pool::ProjectPool;
use sigiro::server::{AppState, build_http_router};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;
use time::{OffsetDateTime, UtcOffset, format_description};

const ROWS_DEFAULT: usize = 20_000;
const ROW_INTERVAL_US: i64 = 300_000_000;

struct TestContext {
    state: Arc<AppState>,
    _data_dir: TempDir,
}

#[derive(Debug)]
struct Diagnostics {
    memory_limit: String,
    memory_usage: String,
}

#[derive(Debug)]
struct QueryMeasurement {
    name: &'static str,
    status: StatusCode,
    elapsed: Duration,
    rows: usize,
    shape: Vec<String>,
    truncated: Option<String>,
    has_truncated_header: bool,
    body: Value,
    diagnostics_before: Diagnostics,
    diagnostics_after: Diagnostics,
    tmp_bytes_before: u64,
    tmp_bytes_after: u64,
}

fn fixture_rows() -> usize {
    std::env::var("SIGIRO_QUERY_MEMORY_ROWS")
        .ok()
        .map(|value| {
            value.parse::<usize>().unwrap_or_else(|error| {
                panic!("SIGIRO_QUERY_MEMORY_ROWS must be an integer: {error}")
            })
        })
        .unwrap_or(ROWS_DEFAULT)
}

fn directory_size(path: &Path) -> u64 {
    let mut total = 0_u64;
    for entry in walkdir::WalkDir::new(path) {
        let entry = entry.unwrap_or_else(|error| panic!("walk {path:?}: {error}"));
        if entry.file_type().is_file() {
            total = total
                .checked_add(
                    entry
                        .metadata()
                        .unwrap_or_else(|error| panic!("stat {:?}: {error}", entry.path()))
                        .len(),
                )
                .expect(".tmp byte count must not overflow");
        }
    }
    total
}

fn timestamp_literal(timestamp_us: i64) -> String {
    let timestamp = OffsetDateTime::from_unix_timestamp_nanos(i128::from(timestamp_us) * 1_000)
        .expect("fixture timestamp must be representable")
        .to_offset(UtcOffset::UTC);
    let format =
        format_description::parse_borrowed::<2>("[year]-[month]-[day] [hour]:[minute]:[second]")
            .expect("valid timestamp format");
    timestamp.format(&format).expect("format fixture timestamp")
}

fn build_test_context() -> TestContext {
    let data_dir = TempDir::new().expect("create isolated DuckLake data directory");
    let data_path = Utf8Path::from_path(data_dir.path()).expect("temporary path is UTF-8");
    let pool = Arc::new(ProjectPool::new(
        data_path.to_owned(),
        4,
        sigiro::storage::StorageMode::LocalFile,
    ));
    pool.create_project("default")
        .expect("initialize default DuckLake project");

    let admin_conn = rusqlite::Connection::open_in_memory().expect("open admin database");
    admin::create_instance_status_table(&admin_conn).expect("create admin status table");
    let admin_conn = Arc::new(parking_lot::Mutex::new(admin_conn));
    let alerts =
        sigiro::alerts::AlertStore::new(Arc::clone(&admin_conn)).expect("initialize alert store");
    let router = Arc::new(MsgRouter::new(Arc::clone(&pool)));
    router.start();

    let state = Arc::new(AppState {
        pool,
        admin_conn,
        alerts,
        http_client: reqwest::Client::new(),
        router,
        auth_mode: sigiro::auth::AuthMode::Open,
        data_dir: data_path.to_owned(),
        init_complete: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        query_surface: sigiro::config::QuerySurface::None,
        query_timeout: std::time::Duration::from_secs(30),
    });
    state.mark_init_complete();
    TestContext {
        state,
        _data_dir: data_dir,
    }
}

fn seed_spans(conn: &Connection, rows: usize, base_us: i64) {
    let sql = format!(
        "INSERT INTO lake.sigiro_spans (
             timestamp, end_timestamp, duration, trace_id, span_id, service_name,
             span_name, span_kind, status_code, resource_attributes, span_attributes,
             events_json, links_json
         )
         SELECT
             make_timestamp({base} + i * {interval}),
             {base} + i * {interval} + 250000,
             250000,
             concat('trace-', i),
             concat('span-', i),
             CASE i % 4 WHEN 0 THEN 'checkout' WHEN 1 THEN 'cart' WHEN 2 THEN 'search' ELSE 'payments' END,
             CASE i % 3 WHEN 0 THEN 'GET /items' WHEN 1 THEN 'POST /checkout' ELSE 'GET /search' END,
             2,
             CAST(i % 4 AS INTEGER),
             json_object(
                 'deployment.environment', CASE i % 2 WHEN 0 THEN 'production' ELSE 'staging' END,
                 'cloud.region', CASE i % 3 WHEN 0 THEN 'eu-west-1' WHEN 1 THEN 'us-east-1' ELSE 'ap-south-1' END,
                 'service.version', concat('2026.01.', i % 12)
             ),
             json_object(
                 'source', concat('agent-', i),
                 'http.method', CASE i % 2 WHEN 0 THEN 'GET' ELSE 'POST' END,
                 'http.route', '/telemetry/query',
                 'request.id', concat('request-', i),
                 'payload', repeat('x', 512)
             ),
             '[{{\"name\":\"cache.lookup\",\"attributes\":{{\"hit\":true}}}}]',
             '[]'
         FROM range({rows}) AS input(i)",
        base = base_us,
        interval = ROW_INTERVAL_US,
        rows = rows
    );
    conn.execute_batch(&sql)
        .expect("seed realistic spans into DuckLake");
}

async fn query_diagnostics(server: &TestServer) -> Diagnostics {
    let response = server
        .post("/v1/query")
        .text("SELECT memory_limit, memory_usage FROM pragma_database_size()")
        .await;
    assert_eq!(
        response.status_code(),
        StatusCode::OK,
        "memory diagnostics query must return HTTP 200; body={}",
        response.text()
    );
    let body: Value = serde_json::from_str(&response.text())
        .unwrap_or_else(|error| panic!("memory diagnostics must be valid JSON: {error}"));
    let row = body
        .as_array()
        .and_then(|rows| rows.first())
        .unwrap_or_else(|| panic!("memory diagnostics must contain one row: {body}"));
    let memory_limit = row
        .get("memory_limit")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("memory diagnostics missing string memory_limit: {row}"));
    let memory_usage = row
        .get("memory_usage")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("memory diagnostics missing string memory_usage: {row}"));
    Diagnostics {
        memory_limit: memory_limit.to_owned(),
        memory_usage: memory_usage.to_owned(),
    }
}

async fn run_query(
    server: &TestServer,
    name: &'static str,
    sql: &str,
    tmp_dir: &Path,
) -> QueryMeasurement {
    let diagnostics_before = query_diagnostics(server).await;
    let tmp_bytes_before = directory_size(tmp_dir);
    let started = Instant::now();
    let response = server.post("/v1/query").text(sql).await;
    let status = response.status_code();
    let truncated = response
        .headers()
        .get("x-sigiro-truncated")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let has_truncated_header = response.headers().contains_key("x-sigiro-truncated");
    let body = response.text();
    let parsed_body = serde_json::from_str::<Value>(&body).ok();
    let rows = parsed_body
        .as_ref()
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let mut shape = parsed_body
        .as_ref()
        .and_then(Value::as_array)
        .and_then(|rows| rows.first())
        .and_then(Value::as_object)
        .map(|row| row.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    shape.sort();
    let diagnostics_after = query_diagnostics(server).await;
    let tmp_bytes_after = directory_size(tmp_dir);
    QueryMeasurement {
        name,
        status,
        elapsed: started.elapsed(),
        rows,
        shape,
        truncated,
        has_truncated_header,
        body: parsed_body.unwrap_or(Value::Null),
        diagnostics_before,
        diagnostics_after,
        tmp_bytes_before,
        tmp_bytes_after,
    }
}

#[tokio::test]
async fn real_ducklake_query_shapes_report_memory_pressure_and_health() {
    let rows = fixture_rows();
    assert!(rows > 0, "fixture must contain at least one span");
    let now_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must be after Unix epoch")
        .as_micros() as i64;
    let fixture_start_us = now_us - 30 * 60 * 1_000_000;
    let ctx = build_test_context();
    let conn = ctx
        .state
        .pool
        .get_project("default")
        .expect("default project exists")
        .get();
    seed_spans(&conn, rows, fixture_start_us);
    drop(conn);

    let tmp_dir = ctx._data_dir.path().join("default").join(".tmp");
    let server = TestServer::new(build_http_router(ctx.state.clone())).expect("build HTTP server");
    let bounded_start = timestamp_literal(fixture_start_us);
    let bounded_end = timestamp_literal(fixture_start_us + 60 * 60 * 1_000_000);
    let queries = [
        (
            "unbounded_narrow_status",
            "SELECT status_code, count(*) AS rows FROM lake.sigiro_spans GROUP BY status_code ORDER BY status_code",
        ),
        (
            "unbounded_json_status_source",
            "SELECT status_code, json_extract_string(span_attributes, '$.source') AS source, count(*) AS rows FROM lake.sigiro_spans GROUP BY 1, 2 ORDER BY source",
        ),
        (
            "wide_json_projection_grouping",
            "SELECT resource_attributes, span_attributes, count(*) AS rows FROM lake.sigiro_spans GROUP BY 1, 2",
        ),
        (
            "bounded_literal_narrow",
            &format!(
                "SELECT status_code, count(*) AS rows FROM lake.sigiro_spans WHERE timestamp >= TIMESTAMP '{bounded_start}' AND timestamp < TIMESTAMP '{bounded_end}' GROUP BY status_code ORDER BY status_code"
            ),
        ),
        (
            "now_pruning_loss_control",
            "SELECT status_code, count(*) AS rows FROM lake.sigiro_spans WHERE timestamp >= (now() - INTERVAL '1 hour')::TIMESTAMP AND timestamp < now()::TIMESTAMP GROUP BY status_code ORDER BY status_code",
        ),
    ];

    let mut measurements = Vec::with_capacity(queries.len());
    for (name, sql) in queries {
        let measurement = run_query(&server, name, sql, &tmp_dir).await;
        if name == "bounded_literal_narrow" {
            assert_eq!(measurement.status, StatusCode::OK, "{name}");
            assert_eq!(
                measurement.truncated.as_deref(),
                Some("false"),
                "successful {name} response must explicitly report no truncation"
            );
            match name {
                "bounded_literal_narrow" => {
                    assert_eq!(measurement.rows, 4);
                    assert_eq!(measurement.shape, ["rows", "status_code"]);
                }
                _ => unreachable!("all query names are handled"),
            }
        } else {
            assert_eq!(
                measurement.status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{name}"
            );
            assert_eq!(measurement.body["code"], "query_requires_time_bounds");
            assert_eq!(measurement.body["retryable"], false);
            assert!(!measurement.has_truncated_header);
        }
        measurements.push(measurement);

        let health = server.get("/health").await;
        health.assert_status_ok();
        let select_one = server.post("/v1/query").text("SELECT 1 AS healthy").await;
        select_one.assert_status_ok();
    }

    for measurement in measurements {
        println!(
            "query-memory case={} status={} elapsed_ms={} rows={} truncated={:?} tmp_bytes_before={} tmp_bytes_after={} memory_before=({}, {}) memory_after=({}, {})",
            measurement.name,
            measurement.status,
            measurement.elapsed.as_secs_f64() * 1000.0,
            measurement.rows,
            measurement.truncated,
            measurement.tmp_bytes_before,
            measurement.tmp_bytes_after,
            measurement.diagnostics_before.memory_limit,
            measurement.diagnostics_before.memory_usage,
            measurement.diagnostics_after.memory_limit,
            measurement.diagnostics_after.memory_usage,
        );
    }
    println!(
        "query-memory fixture rows={rows}; snapshots are per-query before/after diagnostics, not peak memory; default scale is bounded and deterministic"
    );
}
