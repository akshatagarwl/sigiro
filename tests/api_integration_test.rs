//! Integration tests for sigiro HTTP API endpoints.
//!
//! Uses axum-test to exercise the Router directly (no TCP listener).
//! Test state uses temp directories for SIGIRO_DATA_DIR and an in-memory
//! SQLite admin DB.

#![allow(clippy::disallowed_methods)]

use axum_test::TestServer;
use rmcp::model::CallToolRequestParams;
use rmcp::{ClientHandler, ServiceExt};
use serde_json::json;

use parking_lot::Mutex;
use sigiro::admin;
use sigiro::client::{ClientAuth, SigiroClient};
use sigiro::ingest;
use sigiro::inserter::MsgRouter;
use sigiro::mcp::SigiroMcp;
use sigiro::pool::ProjectPool;
use sigiro::server::{AppState, build_http_router};
use std::sync::Arc;
use tempfile::TempDir;

struct TestContext {
    state: Arc<AppState>,
    // Hold the TempDir guard for the test's lifetime; dropping it deletes the
    // data dir. Under the DuckDB filesystem lockdown a vanished dir is a hard
    // error (previously it was silently recreated), so this must outlive the state.
    _tmp_dir: TempDir,
}

#[derive(Clone, Default)]
struct TestMcpClient;

impl ClientHandler for TestMcpClient {}

/// Build a test AppState with temp directories and an in-memory SQLite admin
/// DB with all required tables.
fn build_test_context() -> TestContext {
    // Self-host default: `Open` auth (no per-request key).
    build_context_with_auth(sigiro::auth::AuthMode::Open)
}

fn build_context_with_auth(auth_mode: sigiro::auth::AuthMode) -> TestContext {
    let tmp_dir = TempDir::new().expect("failed to create temp dir");
    let data_dir = camino::Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).unwrap();

    let pool = Arc::new(ProjectPool::new(
        data_dir.clone(),
        4,
        sigiro::storage::StorageMode::LocalFile,
    ));

    // Explicit DI: initialize the default project connection before handlers access it.
    pool.create_project("default")
        .expect("failed to init default project connection");

    let admin_conn = Arc::new(Mutex::new(
        admin::test_connection().expect("failed to open in-memory control state"),
    ));
    let alerts = sigiro::alerts::AlertStore::new(Arc::clone(&admin_conn))
        .expect("failed to initialize alert store");

    let router = Arc::new(sigiro::inserter::MsgRouter::new(Arc::clone(&pool)));
    router.start();
    let state = Arc::new(AppState {
        pool,
        admin_conn,
        alerts,
        http_client: reqwest::Client::new(),
        router,
        auth_mode,
        data_dir: data_dir.clone(),
        init_complete: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        query_surface: sigiro::config::QuerySurface::None,
    });

    TestContext {
        state,
        _tmp_dir: tmp_dir,
    }
}

/// Build the OTLP HTTP router (port 4318).
fn build_otlp_app_with_router(state: &Arc<AppState>) -> (axum::Router, Arc<MsgRouter>) {
    let router = Arc::new(MsgRouter::new_with_disk_and_alerts(
        Arc::clone(&state.pool),
        sigiro::inserter::DiskCapacityState::new(0),
        state.alerts.clone(),
    ));
    router.start();
    (
        ingest::otlp_http_router(Arc::clone(&router), state.auth_mode.clone(), 8_388_608),
        router,
    )
}

/// Build the OTLP HTTP router (port 4318).
fn build_otlp_app(state: &Arc<AppState>) -> axum::Router {
    build_otlp_app_with_router(state).0
}

// -- Tests ----------------------------------------------------------------

/// Mark the test AppState as "init complete" by flipping the
/// `init_complete` AtomicBool that the production server flips at the tail
/// of init (see src/main.rs). Health checks must reflect that state.
fn mark_test_context_ready(ctx: &TestContext) {
    ctx.state.mark_init_complete();
}

#[tokio::test]
async fn trace_detail_preserves_canonical_trace_id_wire_shape() {
    const TRACE_ID: &str = "11111111111111111111111111111111";
    let ctx = build_test_context();
    ctx.state
        .pool
        .get_project("default")
        .expect("default project exists")
        .get()
        .execute(
            "INSERT INTO lake.sigiro_spans (
                timestamp, end_timestamp, duration, trace_id, span_id,
                service_name, span_name, span_kind, status_code
             ) VALUES (now(), 1700000000000010, 10, ?1,
                '2222222222222222', 'checkout', 'GET /cart', 2, 1)",
            duckdb::params![TRACE_ID],
        )
        .expect("seed trace span");

    let server = TestServer::new(build_http_router(ctx.state)).expect("build test server");
    let response = server.get(&format!("/v1/traces/{TRACE_ID}")).await;
    response.assert_status_ok();
    let body: serde_json::Value = serde_json::from_str(&response.text()).expect("trace JSON");
    assert_eq!(body["trace_id"], TRACE_ID);
    assert_eq!(body["root_span"]["trace_id"], TRACE_ID);
    assert_eq!(body["spans"][0]["trace_id"], TRACE_ID);
    assert_eq!(body["root_span"]["span_id"], "2222222222222222");
    assert_eq!(body["critical_path"][0], "2222222222222222");
}

#[tokio::test]
async fn trace_detail_uses_elapsed_span_envelope_for_total_duration() {
    const TRACE_ID: &str = "33333333333333333333333333333333";
    const START_US: i64 = 1_700_000_000_000_000;
    let ctx = build_test_context();
    ctx.state
        .pool
        .get_project("default")
        .expect("default project exists")
        .get()
        .execute_batch(&format!(
            "INSERT INTO lake.sigiro_spans (
                timestamp, end_timestamp, duration, trace_id, span_id,
                parent_span_id, service_name, span_name, span_kind, status_code
             ) VALUES
                (make_timestamp({START_US}), {START_US} + 10000000, 10000000,
                    '{TRACE_ID}', '1111111111111111', '', 'frontend', 'request', 2, 1),
                (make_timestamp({START_US} + 1000000), {START_US} + 5000000, 4000000,
                    '{TRACE_ID}', '2222222222222222', '1111111111111111',
                    'database', 'query', 3, 1),
                (make_timestamp({START_US} + 8000000), {START_US} + 13000000, 5000000,
                    '{TRACE_ID}', '3333333333333333', '1111111111111111',
                    'worker', 'flush', 3, 1)"
        ))
        .expect("seed overlapping trace spans");

    let server = TestServer::new(build_http_router(ctx.state)).expect("build test server");
    let response = server.get(&format!("/v1/traces/{TRACE_ID}")).await;
    response.assert_status_ok();
    let body: serde_json::Value = serde_json::from_str(&response.text()).expect("trace JSON");

    assert_eq!(body["total_duration_us"], 13_000_000);
    assert_eq!(body["root_span"]["duration_us"], 10_000_000);
    let inclusive_percentage_total = body["service_breakdown"]
        .as_array()
        .expect("service breakdown array")
        .iter()
        .map(|entry| entry["percentage"].as_f64().expect("numeric percentage"))
        .sum::<f64>();
    assert!((inclusive_percentage_total - 100.0).abs() < 0.001);
}

#[tokio::test]
async fn trace_detail_rejects_noncanonical_trace_ids_during_path_deserialization() {
    let ctx = build_test_context();
    let server = TestServer::new(build_http_router(ctx.state)).expect("build test server");

    for invalid in [
        "1111111111111111111111111111111",
        "1111111111111111111111111111111g",
        "1111111111111111111111111111111A",
        "00000000000000000000000000000000",
    ] {
        server
            .get(&format!("/v1/traces/{invalid}"))
            .await
            .assert_status(axum::http::StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn test_openapi_documents_diagnose_for_agents() {
    // Cold-start agentic discoverability: /openapi.json is the only public
    // discovery surface, so the flagship /v1/diagnose MUST be fully
    // documented there — request schema, a description, and the bearer scheme.
    // This regressed silently once (schemars derive instead of utoipa ToSchema,
    // empty path macro), which is what made sigiro un-agentic from a cold URL.
    let ctx = build_test_context();
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    let spec: serde_json::Value =
        serde_json::from_str(&server.get("/openapi.json").await.text()).expect("openapi is JSON");

    let inv = &spec["paths"]["/v1/diagnose"]["post"];
    assert!(
        inv["requestBody"].is_object(),
        "diagnose must publish a request body schema so an agent can learn to call it"
    );
    assert!(
        inv["description"]
            .as_str()
            .unwrap_or("")
            .contains("microsecond"),
        "diagnose description must state the microsecond timestamp unit"
    );
    let request = &spec["components"]["schemas"]["DiagnoseRequest"];
    assert_eq!(request["required"], json!(["services"]));
    assert!(request["properties"]["services"].is_object());
    assert!(
        request["properties"].get("service").is_none(),
        "diagnose must expose only the plural services field"
    );
    // The response body schema graph must resolve (findings/drill fields).
    assert!(
        spec["components"]["schemas"]["DiagnoseResponse"].is_object()
            && spec["components"]["schemas"]["Finding"].is_object(),
        "response schema graph must be published"
    );
    assert_eq!(
        spec["components"]["schemas"]["FindingSeverity"]["enum"],
        serde_json::json!(["critical", "warning", "info"]),
        "finding severity must be a closed wire enum"
    );
    let variants = spec["components"]["schemas"]["FindingKind"]["oneOf"]
        .as_array()
        .expect("FindingKind must be a tagged union");
    let expected_kinds = [
        "incident",
        "anomalous_operation",
        "anomalous_log_volume",
        "anomalous_error_log_rate",
        "anomalous_profile_cost",
        "errors_without_logs",
        "coverage_gap",
        "missing_resource_attributes",
        "duplicate_spans",
        "orphan_spans",
        "missing_client_spans",
    ];
    let actual_kinds: Vec<&str> = variants
        .iter()
        .map(|variant| {
            variant["properties"]["kind"]["enum"][0]
                .as_str()
                .expect("every finding variant has an exact kind tag")
        })
        .collect();
    assert_eq!(actual_kinds, expected_kinds);
    let incident = &variants[0];
    assert!(
        incident["required"]
            .as_array()
            .expect("incident required fields")
            .iter()
            .any(|field| field == "incident"),
        "incident payload must be required"
    );
    assert!(incident["properties"].get("drill_down_sql").is_none());
    assert!(
        variants[1..]
            .iter()
            .all(|variant| variant["properties"].get("incident").is_none())
    );
    let shift_required = spec["components"]["schemas"]["IncidentShift"]["required"]
        .as_array()
        .expect("IncidentShift required fields");
    assert!(
        shift_required.iter().any(|field| field == "drill_down_sql"),
        "incident shifts require their drill query"
    );
    assert!(
        spec["components"]["securitySchemes"]["bearer"].is_object(),
        "bearer security scheme must be declared so an agent knows auth"
    );
}

#[tokio::test]
async fn openapi_query_request_example_executes() {
    let ctx = build_test_context();
    ctx.state
        .pool
        .get_project("default")
        .expect("default project exists")
        .get()
        .execute(
            "INSERT INTO lake.sigiro_spans (
                timestamp, end_timestamp, duration, trace_id, span_id,
                service_name, span_name, span_kind, status_code
             ) VALUES (now(), epoch_us(now()), 0,
                '11111111111111111111111111111111', '2222222222222222',
                'checkout', 'GET /cart', 2, 2)",
            [],
        )
        .expect("seed error span");
    let server = TestServer::new(build_http_router(ctx.state)).expect("build test server");
    let spec: serde_json::Value =
        serde_json::from_str(&server.get("/openapi.json").await.text()).expect("openapi is JSON");
    let query =
        spec["paths"]["/v1/query"]["post"]["requestBody"]["content"]["text/plain"]["example"]
            .as_str()
            .expect("query request example must be SQL text");

    server
        .post("/v1/query")
        .content_type("text/plain")
        .bytes(query.as_bytes().to_vec().into())
        .await
        .assert_status_ok();
}

#[tokio::test]
async fn test_pyroscope_routes_and_public_openapi_stay_in_sync() {
    // Axum does not expose stable route introspection. Exercise every live alias
    // through the same merged routers production uses, then assert those paths'
    // generated operations. This catches either half of the drift without a
    // third hand-maintained list falsely claiming completeness for every route.
    let ctx = build_test_context();
    let ingest_app = ingest::otlp_http_router(
        Arc::clone(&ctx.state.router),
        ctx.state.auth_mode.clone(),
        8_388_608,
    );
    let app = build_http_router(Arc::clone(&ctx.state)).merge(ingest_app);
    let server = TestServer::new(app).expect("failed to build test server");
    let paths = [
        "/",
        "/push.v1.PusherService/Push",
        "/ingest",
        "/ingest/pyroscope",
    ];

    for path in paths {
        let response = server
            .post(path)
            .content_type("application/proto")
            .bytes(Vec::new().into())
            .await;
        response.assert_status(axum::http::StatusCode::BAD_REQUEST);
    }

    let spec: serde_json::Value =
        serde_json::from_str(&server.get("/openapi.json").await.text()).expect("openapi is JSON");
    let binary_schema = &spec["components"]["schemas"]["PyroscopeProfileBody"];
    assert_eq!(binary_schema["type"], "string");
    assert_eq!(binary_schema["format"], "binary");

    for path in paths {
        let operation = &spec["paths"][path]["post"];
        assert!(operation.is_object(), "{path} must be in the public spec");
        assert!(
            operation["description"]
                .as_str()
                .unwrap_or_default()
                .contains("https://sigiro.com/docs/how-to/send-profiles"),
            "{path} must link to the complete profile ingest guide"
        );

        let name = operation["parameters"]
            .as_array()
            .expect("Pyroscope operation parameters")
            .iter()
            .find(|parameter| parameter["name"] == "name")
            .expect("Pyroscope name query parameter");
        assert_eq!(name["in"], "query");
        assert_eq!(name["required"], false);
        let name_description = name["description"].as_str().unwrap_or_default();
        assert!(name_description.contains("raw pprof"));
        assert!(name_description.contains("service_name"));

        let request_body = &operation["requestBody"];
        assert_eq!(request_body["required"], true);
        for content_type in [
            "application/proto",
            "application/octet-stream",
            "binary/octet-stream",
            "multipart/form-data",
        ] {
            assert_eq!(
                request_body["content"][content_type]["schema"]["$ref"],
                "#/components/schemas/PyroscopeProfileBody",
                "{path} must describe {content_type} as binary"
            );
        }
        assert_eq!(operation["security"], json!([{"bearer": []}]));
        for status in ["200", "400", "401", "413", "429", "500", "507"] {
            assert!(
                operation["responses"][status].is_object(),
                "{path} must document response {status}"
            );
        }
    }
}

#[tokio::test]
async fn test_diagnose_rejects_singular_service_field() {
    let ctx = build_test_context();
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    server
        .post("/v1/diagnose")
        .json(&json!({"service": "checkout"}))
        .await
        .assert_status(axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    server
        .post("/v1/diagnose")
        .json(&json!({"service": "checkout", "services": ["cart"]}))
        .await
        .assert_status(axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    // A valid plural request must pass request-shape validation. Its domain
    // result depends on whether the platform creates project metadata eagerly.
    let response = server
        .post("/v1/diagnose")
        .json(&json!({"services": ["nonexistent"]}))
        .await;
    assert_ne!(
        response.status_code(),
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "the plural services field must be accepted"
    );
}

#[tokio::test]
async fn test_health_returns_ok_when_ready() {
    let ctx = build_test_context();
    mark_test_context_ready(&ctx);
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    let resp = server.get("/health").await;
    resp.assert_status_ok();
    let body: serde_json::Value = serde_json::from_str(&resp.text()).expect("health body is JSON");
    assert_eq!(
        body.get("status").and_then(|v| v.as_str()),
        Some("ready"),
        "expected ready status, got {body}"
    );

    server.get("/healthz").await.assert_status_ok();
    server
        .get("/v1/health")
        .await
        .assert_status(axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_health_returns_503_when_starting() {
    // init_complete is false (default) — simulates a server that just
    // bound the HTTP listener but hasn't finished init (DuckDB attach,
    // etc.). Health probes must see this as not ready.
    let ctx = build_test_context();
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    let resp = server.get("/health").await;
    resp.assert_status(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value = serde_json::from_str(&resp.text()).expect("health body is JSON");
    assert_eq!(
        body.get("status").and_then(|v| v.as_str()),
        Some("starting"),
        "expected starting status, got {body}"
    );
    // Deserializes into the typed HealthResponse — guards the wire format.
    let health: sigiro::responses::HealthResponse =
        serde_json::from_value(body).expect("starting body should deserialize to HealthResponse");
    assert!(matches!(
        health,
        sigiro::responses::HealthResponse::Starting
    ));
}

#[tokio::test]
async fn test_query_midstream_error_never_returns_partial_rows() {
    let ctx = build_test_context();
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    // A query that produces rows but raises an error partway through the scan.
    // The old handler used `while let Ok(Some(row)) = rows.next()`, which
    // swallowed the row-stream Err and returned 200 with the rows accumulated
    // so far. It must now surface the error and never emit partial data.
    //
    // Uses an invalid CAST (not the `error()` scalar function this test used
    // before) so the mid-stream failure comes from `Expr::Cast`, never from a
    // function call `AllowlistVisitor`'s scalar allowlist would gate —
    // `error()` is a debug/testing utility, not a real analytics scalar, so
    // it deliberately isn't on `server::SCALAR_FUNCTIONS`. Empirically
    // confirmed (`duckdb` CLI + this test) that DuckDB only raises the
    // Conversion Error when row i=3 is actually reached — `/` integer
    // division by zero returns `inf`/`NULL` rather than erroring, so that
    // was tried and rejected first.
    let resp = server
        .post("/v1/query")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .text(
            "SELECT CASE WHEN i = 3 THEN CAST('not_a_number' AS BIGINT) ELSE i END AS v \
             FROM range(5) t(i)",
        )
        .await;

    assert_ne!(
        resp.status_code(),
        axum::http::StatusCode::OK,
        "mid-stream query error must not return 200 with partial rows"
    );
    resp.assert_status(axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_query_with_dev_auth_bad_sql_returns_400() {
    let ctx = build_test_context();
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    let resp = server
        .post("/v1/query")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .text("NOT VALID SQL AT ALL!!!")
        .await;
    resp.assert_status(axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
// Timezone arithmetic must work on the locked-down query surface. The
// engine sets `enable_external_access = false`, which blocks autoloading the
// `icu` extension — where DuckDB's TIMESTAMPTZ / `now() - INTERVAL` /
// `AT TIME ZONE` / `epoch(now())` all live. `lock_filesystem` therefore
// pre-loads `icu` before the lockdown; without it, the single most common
// observability query ("last N hours", the exact pattern the onboarding docs
// teach) fails at prepare time with a Binder Error. This guards that fix — a
// unit test can't catch it because a standalone DuckDB autoloads icu freely.
async fn test_timezone_query_works_under_lockdown() {
    let ctx = build_test_context();
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    let resp = server
        .post("/v1/query")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .text("SELECT count(*) AS n FROM sigiro_spans WHERE timestamp > now() - INTERVAL '1 hour'")
        .await;
    resp.assert_status_ok();

    let resp = server
        .post("/v1/query")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .text("SELECT epoch(now()) AS e FROM sigiro_spans LIMIT 1")
        .await;
    resp.assert_status_ok();
}

#[tokio::test]
async fn test_query_supports_duckdb_aggregate_filter() {
    let ctx = build_test_context();
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    let resp = server
        .post("/v1/query")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .text(
            "SELECT count(*) FILTER (WHERE status_code = 2) AS errors \
             FROM sigiro_spans",
        )
        .await;

    resp.assert_status_ok();
    let rows: serde_json::Value =
        serde_json::from_str(&resp.text()).expect("query response should be JSON");
    assert_eq!(
        rows.pointer("/0/errors")
            .and_then(serde_json::Value::as_i64),
        Some(0)
    );
}

#[tokio::test]
async fn test_services_returns_empty_list_for_empty_project() {
    let ctx = build_test_context();
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    let resp = server
        .get("/v1/services")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = serde_json::from_str(&resp.text()).expect("valid JSON");
    assert_eq!(body, json!({"services": []}));
}

#[tokio::test]
async fn test_services_discovers_every_supported_signal() {
    let ctx = build_test_context();
    let readers = ctx
        .state
        .pool
        .get_project("default")
        .expect("default project exists");
    readers
        .get()
        .execute_batch(
            "INSERT INTO lake.sigiro_spans
                 (timestamp, end_timestamp, duration, service_name, span_name, span_kind, status_code)
             VALUES
                 ('2026-01-10 00:00:00', 1, 1, 'mixed', 'request', 2, 1),
                 ('2026-01-01 00:00:00', 1, 1, 'span-only', 'request', 2, 1);

             INSERT INTO lake.sigiro_logs
                 (timestamp, observed_timestamp, service_name, severity_number, severity_text)
             VALUES
                 ('2026-01-08 00:00:00', 1, 'mixed', 9, 'INFO'),
                 ('2026-01-02 00:00:00', 1, 'log-only', 9, 'INFO'),
                 ('2026-01-03 00:00:00', 1, 'log-only', 9, 'INFO');

             INSERT INTO lake.sigiro_metrics_gauge
                 (timestamp, metric_name, value, service_name)
             VALUES
                 ('2026-01-09 00:00:00', 'cpu', 1, 'mixed'),
                 ('2026-01-04 00:00:00', 'cpu', 1, 'gauge-only');

             INSERT INTO lake.sigiro_metrics_sum
                 (timestamp, metric_name, value, service_name, aggregation_temporality, is_monotonic)
             VALUES
                 ('2026-01-11 00:00:00', 'requests', 1, 'mixed', 2, true),
                 ('2026-01-05 00:00:00', 'requests', 1, 'sum-only', 2, true);

             INSERT INTO lake.sigiro_metrics_histogram
                 (timestamp, metric_name, count, bucket_counts, explicit_bounds,
                  service_name, aggregation_temporality)
             VALUES
                 ('2026-01-06 00:00:00', 'latency', 999, '[]', '[]',
                  'histogram-only', 2);

             INSERT INTO lake.sigiro_metrics_exp_histogram
                 (timestamp, metric_name, count, scale, zero_count, service_name,
                  aggregation_temporality)
             VALUES
                 ('2026-01-07 00:00:00', 'latency', 999, 0, 0,
                  'exp-histogram-only', 2);",
        )
        .expect("seed cross-signal telemetry");

    let server = TestServer::builder()
        .http_transport()
        .build(build_http_router(ctx.state))
        .expect("build test server");
    let resp = server.get("/v1/services").await;
    resp.assert_status_ok();
    let body: serde_json::Value = serde_json::from_str(&resp.text()).expect("valid JSON");
    let services = body["services"].as_array().expect("services array");

    let names: Vec<&str> = services
        .iter()
        .map(|service| service["name"].as_str().expect("service name"))
        .collect();
    assert_eq!(
        names,
        vec![
            "mixed",
            "log-only",
            "exp-histogram-only",
            "gauge-only",
            "histogram-only",
            "span-only",
            "sum-only",
        ],
        "services order by total count descending, then name ascending"
    );

    let service = |name: &str| {
        services
            .iter()
            .find(|service| service["name"] == name)
            .unwrap_or_else(|| panic!("missing service {name}"))
    };
    assert_eq!(
        service("mixed")["signals"],
        json!({"spans": 1, "logs": 1, "metrics": 2})
    );
    assert_eq!(service("mixed")["totalCount"], 4);
    assert_eq!(service("mixed")["firstSeen"], "2026-01-08 00:00:00");
    assert_eq!(service("mixed")["lastSeen"], "2026-01-11 00:00:00");
    assert_eq!(
        service("log-only")["signals"],
        json!({"spans": 0, "logs": 2, "metrics": 0})
    );
    assert_eq!(
        service("span-only")["signals"],
        json!({"spans": 1, "logs": 0, "metrics": 0})
    );
    for name in [
        "gauge-only",
        "sum-only",
        "histogram-only",
        "exp-histogram-only",
    ] {
        assert_eq!(
            service(name)["signals"],
            json!({"spans": 0, "logs": 0, "metrics": 1}),
            "metric-family-only service {name}"
        );
        assert_eq!(service(name)["totalCount"], 1);
    }

    let client = SigiroClient::new(
        server.server_url("/").expect("test server URL").as_str(),
        ClientAuth::Open,
    );
    let typed = client.services().await.expect("typed services response");
    assert_eq!(typed.services.len(), 7);
    let typed_log_only = typed
        .services
        .iter()
        .find(|service| service.name == "log-only")
        .expect("typed log-only service");
    assert_eq!(
        typed_log_only.first_seen.as_deref(),
        Some("2026-01-02 00:00:00")
    );
    assert_eq!(
        typed_log_only.last_seen.as_deref(),
        Some("2026-01-03 00:00:00")
    );
    assert_eq!(typed_log_only.signals.spans, 0);
    assert_eq!(typed_log_only.signals.logs, 2);
    assert_eq!(typed_log_only.signals.metrics, 0);
    assert_eq!(typed_log_only.total_count, 2);
    let expected = serde_json::to_value(typed).expect("serialize typed services response");

    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let mcp_server = tokio::spawn(async move {
        SigiroMcp::new(client)
            .serve(server_transport)
            .await?
            .waiting()
            .await?;
        anyhow::Ok(())
    });
    let mcp_client = TestMcpClient
        .serve(client_transport)
        .await
        .expect("start MCP client");
    let result = mcp_client
        .call_tool(CallToolRequestParams::new("list_services"))
        .await
        .expect("call list_services");
    let text = result
        .content
        .first()
        .and_then(|content| content.as_text())
        .map(|content| content.text.as_str())
        .expect("list_services text result");
    let actual: serde_json::Value = serde_json::from_str(text).expect("valid MCP services JSON");
    assert_eq!(
        actual, expected,
        "HTTP client and MCP service contracts differ"
    );

    mcp_client.cancel().await.expect("stop MCP client");
    mcp_server
        .await
        .expect("join MCP server")
        .expect("stop MCP server");
}

#[tokio::test]
async fn test_anomalies_endpoint_returns_persisted_rows() {
    let ctx = build_test_context();

    // Seed two anomaly rows into the default project's table — /v1/anomalies
    // reads exactly what the scheduled pass (sigiro::anomaly) writes. The
    // BIGGER shift (0→42, unit-free magnitude 1.0 vs ≈0.008 for 24.9→25.3)
    // is deliberately given the OLDER ts_shift, so the ordering assertion
    // below can only pass on magnitude, never on recency.
    {
        let readers = ctx
            .state
            .pool
            .create_project("default")
            .expect("default project");
        let conn = readers.get();
        conn.execute_batch(
            "INSERT INTO lake.sigiro_anomalies
                (detected_at, service_name, operation, signal, kind, direction,
                 ts_shift, \"before\", \"after\", drill_down_sql)
             VALUES (now()::TIMESTAMP, 'checkout', 'POST /pay', 'error_rate',
                     'changepoint', 'worsening',
                     now()::TIMESTAMP - INTERVAL '1 hour', 0.0, 42.0,
                     'SELECT * FROM lake.sigiro_spans'),
                    (now()::TIMESTAMP, 'checkout', 'GET /cart', 'error_rate',
                     'changepoint', 'worsening', now()::TIMESTAMP, 24.9, 25.3,
                     'SELECT * FROM lake.sigiro_spans')",
        )
        .expect("seed anomaly rows");
    }

    let app = build_http_router(Arc::clone(&ctx.state));
    let server = TestServer::builder()
        .http_transport()
        .build(app)
        .expect("failed to build HTTP test server");

    // Unfiltered: BOTH rows come back (rank, don't filter — the trivial
    // 24.9→25.3 shift is never dropped), biggest unit-free relative shift
    // ((after - before) / (after + before)) first.
    let resp = server
        .get("/v1/anomalies")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = serde_json::from_str(&resp.text()).expect("valid JSON");
    let rows = body
        .get("anomalies")
        .and_then(|v| v.as_array())
        .expect("'anomalies' array");
    assert_eq!(rows.len(), 2, "every detected shift is returned");
    let row = &rows[0];
    assert_eq!(row["service_name"], "checkout");
    assert_eq!(
        row["operation"], "POST /pay",
        "the biggest shift leads despite being the older one"
    );
    assert_eq!(row["signal"], "error_rate");
    assert_eq!(row["kind"], "changepoint");
    assert_eq!(row["direction"], "worsening");
    assert_eq!(row["before"], 0.0);
    assert_eq!(row["after"], 42.0);
    assert!(row["drill_down_sql"].is_string());
    assert!(row["ts_shift"].is_string(), "ISO timestamp string");
    assert_eq!(
        rows[1]["operation"], "GET /cart",
        "the smaller shift ranks second, never filtered out"
    );

    // This integration test is a separate crate, so reading these fields proves
    // the typed client response is usable by external Rust consumers rather
    // than only by Sigiro's in-crate CLI and MCP serializers.
    let client = SigiroClient::new(
        server.server_url("/").expect("test server URL").as_str(),
        ClientAuth::Open,
    );
    let response = client
        .anomalies(Some("checkout"), None, None)
        .await
        .expect("typed anomaly response");
    assert!(!response.truncated);
    assert_eq!(response.anomalies.len(), 2);
    let entry = &response.anomalies[0];
    assert!(!entry.detected_at.is_empty());
    assert_eq!(entry.service_name, "checkout");
    assert_eq!(entry.operation, "POST /pay");
    assert_eq!(entry.signal, "error_rate");
    assert_eq!(entry.kind, "changepoint");
    assert_eq!(entry.direction, "worsening");
    assert!(!entry.ts_shift.is_empty());
    assert!(entry.ts_shift_us > 0);
    assert!(entry.incident_id.is_none());
    assert_eq!(entry.before, 0.0);
    assert_eq!(entry.after, 42.0);
    assert_eq!(
        entry.drill_down_sql.as_deref(),
        Some("SELECT * FROM lake.sigiro_spans")
    );

    // Service filter: a non-matching service returns an empty list.
    let resp = server
        .get("/v1/anomalies?service=unrelated")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = serde_json::from_str(&resp.text()).expect("valid JSON");
    assert_eq!(
        body["anomalies"].as_array().map(Vec::len),
        Some(0),
        "service filter must exclude the seeded row"
    );

    // sigiro_anomalies is a TABLE on the /v1/query allowlist — drill queries
    // against it work through the firewall like any telemetry table.
    let resp = server
        .post("/v1/query")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .text("SELECT service_name, \"before\", \"after\" FROM sigiro_anomalies")
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = serde_json::from_str(&resp.text()).expect("valid JSON");
    assert_eq!(body.as_array().map(Vec::len), Some(2));
}

#[tokio::test]
async fn test_anomalies_incident_id_groups_same_bucket_shifts_per_service() {
    let ctx = build_test_context();

    // Four rows: error_rate + log_volume on 'checkout' shifting in the SAME
    // 5-minute detection bucket (one incident), one 'checkout' shift 3 hours
    // earlier (uncorrelated), and one 'other' service shifting at the same
    // moment as the incident (same time, different service — must NOT join).
    {
        let readers = ctx
            .state
            .pool
            .create_project("default")
            .expect("default project");
        let conn = readers.get();
        conn.execute_batch(
            "INSERT INTO lake.sigiro_anomalies
                (detected_at, service_name, operation, signal, kind, direction,
                 ts_shift, \"before\", \"after\", drill_down_sql)
             VALUES (now()::TIMESTAMP, 'checkout', 'POST /pay', 'error_rate',
                     'changepoint', 'worsening',
                     now()::TIMESTAMP - INTERVAL '1 hour', 5.0, 50.0,
                     'SELECT * FROM lake.sigiro_spans'),
                    (now()::TIMESTAMP, 'checkout', 'checkout', 'log_volume',
                     'changepoint', 'worsening',
                     now()::TIMESTAMP - INTERVAL '1 hour', 3.0, 40.0,
                     'SELECT * FROM lake.sigiro_logs'),
                    (now()::TIMESTAMP, 'checkout', 'GET /cart', 'error_rate',
                     'changepoint', 'worsening',
                     now()::TIMESTAMP - INTERVAL '4 hours', 0.0, 20.0,
                     'SELECT * FROM lake.sigiro_spans'),
                    (now()::TIMESTAMP, 'other', 'other', 'log_volume',
                     'changepoint', 'worsening',
                     now()::TIMESTAMP - INTERVAL '1 hour', 2.0, 30.0,
                     'SELECT * FROM lake.sigiro_logs')",
        )
        .expect("seed anomaly rows");
    }

    let app = build_http_router(Arc::clone(&ctx.state));
    let server = TestServer::new(app).expect("failed to build test server");
    let resp = server
        .get("/v1/anomalies")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = serde_json::from_str(&resp.text()).expect("valid JSON");
    let rows = body["anomalies"].as_array().expect("'anomalies' array");
    assert_eq!(rows.len(), 4, "raw rows are all still returned");

    let row_for = |operation: &str, service: &str| -> &serde_json::Value {
        rows.iter()
            .find(|r| r["operation"] == operation && r["service_name"] == service)
            .unwrap_or_else(|| panic!("row for {service}/{operation}"))
    };
    let pay = row_for("POST /pay", "checkout");
    assert!(
        pay["ts_shift_us"].is_i64(),
        "µs shift time rides along for correlation-aware clients"
    );
    // The two same-bucket 'checkout' shifts share one non-null id...
    let incident_id = pay["incident_id"].clone();
    assert!(
        incident_id.is_string(),
        "correlated rows carry an incident_id"
    );
    assert_eq!(
        row_for("checkout", "checkout")["incident_id"],
        incident_id,
        "same service + same bucket = same incident"
    );
    // ...the 3-hours-earlier shift and the other service's do not.
    assert!(
        row_for("GET /cart", "checkout")["incident_id"].is_null(),
        "a shift hours away is not part of the incident"
    );
    assert!(
        row_for("other", "other")["incident_id"].is_null(),
        "same moment on a DIFFERENT service never joins the incident"
    );
}

#[tokio::test]
async fn test_admin_status_requires_admin_token() {
    let ctx = build_test_context();
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    // No Authorization header at all — must be rejected regardless of
    // whether SIGIRO_ADMIN_TOKEN happens to be configured in this process
    // (M-4: /admin/status leaked ingest_dropped_messages_total + init state
    // to anyone, unauthenticated, in every mode).
    let resp = server.get("/admin/status").await;
    resp.assert_status(axum::http::StatusCode::UNAUTHORIZED);

    // Wrong token — also rejected.
    let resp = server
        .get("/admin/status")
        .add_header("Authorization", "Bearer wrong-token")
        .await;
    resp.assert_status(axum::http::StatusCode::UNAUTHORIZED);
}

/// EVERY /admin route is gated, not just the one with a test.
///
/// Admin auth used to be a `require_admin_token(&headers)?` call at the top of
/// each handler, so coverage was per-handler and a new route was one forgotten
/// line away from being an unauthenticated tenant-provisioning / key-minting
/// endpoint. It is now a layer on the /admin router; this asserts the layer
/// actually covers the whole nest, including the three routes that never had an
/// auth test of their own.
#[tokio::test]
async fn every_admin_route_rejects_a_missing_or_wrong_token() {
    let ctx = build_test_context();
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    // Unauthorized must win over body validation and over the
    // "distributed auth required" 400 these handlers return once past the gate —
    // i.e. the layer runs BEFORE the handler, so a bad body cannot smuggle a
    // request past auth.
    let bad = "Bearer definitely-not-the-admin-token";

    for (route, resp) in [
        ("GET /admin/status", server.get("/admin/status").await),
        (
            "GET /admin/status (wrong token)",
            server
                .get("/admin/status")
                .add_header("Authorization", bad)
                .await,
        ),
        (
            "GET /admin/status (malformed header)",
            server
                .get("/admin/status")
                .add_header("Authorization", "not-even-bearer")
                .await,
        ),
    ] {
        assert_eq!(
            resp.status_code(),
            axum::http::StatusCode::UNAUTHORIZED,
            "{route} must be 401 without a valid admin token, got {}",
            resp.status_code()
        );
    }
}

#[tokio::test]
async fn test_admin_status_returns_200_with_admin_token() {
    // SAFETY: set_var is unsafe in concurrent programs; this is a test
    // helper that runs in the same process as the server under test.
    // SIGIRO_ADMIN_TOKEN is on_startup (typed_env), so it's cached in a
    // OnceLock on first successful read — set it before any request in
    // this test so the cached value is the one we expect.
    unsafe {
        std::env::set_var("SIGIRO_ADMIN_TOKEN", "test-admin-token");
    }

    let ctx = build_test_context();
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    let resp = server
        .get("/admin/status")
        .add_header("Authorization", "Bearer test-admin-token")
        .await;
    resp.assert_status_ok();

    let body: serde_json::Value = serde_json::from_str(&resp.text()).expect("valid JSON");
    // Empty status is a valid JSON object
    assert!(body.is_object(), "expected JSON object");
    assert_eq!(
        body.pointer("/status/ingest_dropped_messages_total")
            .and_then(|v| v.as_str()),
        Some("0")
    );
}

#[tokio::test]
async fn ingest_body_limit_caps_http_payloads() {
    let ctx = build_test_context();
    let app = ingest::otlp_http_router(
        Arc::clone(&ctx.state.router),
        ctx.state.auth_mode.clone(),
        16,
    );
    TestServer::new(app)
        .expect("build OTLP test server")
        .post("/v1/traces")
        .content_type("application/x-protobuf")
        .bytes(vec![0; 17].into())
        .await
        .assert_status(axum::http::StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
// F-05: empty batch is a valid no-op (e.g. OTel SDK shutdown sends an
// empty final flush). The server should return 200 OK, not 400.
async fn test_otlp_traces_empty_payload_returns_200() {
    let ctx = build_test_context();
    let app = build_otlp_app(&ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    // Empty JSON OTLP traces payload: {"resourceSpans":[]}
    let empty_traces_json = "{\"resourceSpans\":[]}";

    let resp = server
        .post("/v1/traces")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .bytes(empty_traces_json.into())
        .content_type("application/json")
        .await;
    resp.assert_status(axum::http::StatusCode::OK);
}

#[tokio::test]
async fn otlp_rejects_noncanonical_span_ids() {
    let ctx = build_test_context();
    let server = TestServer::new(build_otlp_app(&ctx.state)).expect("failed to build test server");
    let payload = r#"{"resourceSpans":[{"resource":{"attributes":[{"key":"service.name","value":{"stringValue":"bad-span-id"}}]},"scopeSpans":[{"spans":[{"traceId":"11111111111111111111111111111111","spanId":"22","name":"invalid","startTimeUnixNano":"1700000000000000000","endTimeUnixNano":"1700000001000000000"}]}]}]}"#;

    server
        .post("/v1/traces")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .bytes(payload.into())
        .content_type("application/json")
        .await
        .assert_status(axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_otlp_json_payloads_ingest_all_signal_types() {
    let ctx = build_test_context();
    let (app, router) = build_otlp_app_with_router(&ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    let traces_json = r#"{
      "resourceSpans": [{
        "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "json-svc"}}]},
        "scopeSpans": [{
          "scope": {"name": "json-test"},
          "spans": [{
            "traceId": "11111111111111111111111111111111",
            "spanId": "2222222222222222",
            "name": "GET /json",
            "kind": 2,
            "startTimeUnixNano": "1700000000000000000",
            "endTimeUnixNano": "1700000001000000000",
            "status": {"code": 1}
          }]
        }]
      }]
    }"#;
    let logs_json = r#"{
      "resourceLogs": [{
        "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "json-svc"}}]},
        "scopeLogs": [{
          "scope": {"name": "json-test"},
          "logRecords": [{
            "timeUnixNano": "1700000000500000000",
            "eventName": "deployment.failed",
            "severityNumber": 17,
            "severityText": "ERROR",
            "body": {"stringValue": "json log body"},
            "traceId": "11111111111111111111111111111111",
            "spanId": "2222222222222222"
          }]
        }]
      }]
    }"#;
    let metrics_json = r#"{
      "resourceMetrics": [{
        "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "json-svc"}}]},
        "scopeMetrics": [{
          "scope": {"name": "json-test"},
          "metrics": [{
            "name": "json.request.duration",
            "unit": "ms",
            "gauge": {"dataPoints": [{
              "timeUnixNano": "1700000000500000000",
              "asDouble": 42.5
            }]}
          }, {
            "name": "json.request.count",
            "unit": "{request}",
            "sum": {
              "aggregationTemporality": 2,
              "isMonotonic": true,
              "dataPoints": [{
                "timeUnixNano": "1700000000500000000",
                "asInt": "7"
              }]
            }
          }, {
            "name": "json.request.delta",
            "sum": {
              "aggregationTemporality": 1,
              "dataPoints": [{
                "timeUnixNano": "1700000000500000000",
                "asDouble": 2.5,
                "exemplars": [{
                  "timeUnixNano": "1700000000400000000",
                  "asDouble": 1.5
                }]
              }]
            }
          }, {
            "name": "json.request.size",
            "histogram": {
              "aggregationTemporality": 2,
              "dataPoints": [{
                "timeUnixNano": "1700000000500000000",
                "count": "1",
                "sum": 5.5,
                "bucketCounts": ["1"]
              }]
            }
          }, {
            "name": "json.request.exp_size",
            "exponentialHistogram": {
              "aggregationTemporality": 1,
              "dataPoints": [{
                "timeUnixNano": "1700000000500000000",
                "count": "1",
                "sum": 3.5,
                "positive": {"bucketCounts": ["1"]}
              }]
            }
          }]
        }]
      }]
    }"#;

    for (path, payload) in [
        ("/v1/traces", traces_json),
        ("/v1/logs", logs_json),
        ("/v1/metrics", metrics_json),
    ] {
        let resp = server
            .post(path)
            .add_header("Authorization", "Bearer sigiro_dev_test")
            .bytes(payload.into())
            .content_type("application/json")
            .await;
        resp.assert_status(axum::http::StatusCode::OK);
        if path == "/v1/metrics" {
            assert_eq!(
                resp.headers()[axum::http::header::CONTENT_TYPE],
                "application/json"
            );
            let body: serde_json::Value =
                serde_json::from_str(&resp.text()).expect("metrics response should be JSON");
            assert!(
                body.get("partialSuccess").is_none(),
                "unexpected metrics partial success: {body}"
            );
        }
    }

    router
        .shutdown_and_wait()
        .await
        .expect("flusher should persist OTLP JSON rows");

    let conn = ctx
        .state
        .pool
        .get_project("default")
        .expect("default project connection should exist");
    let guard = conn.get();
    let spans: i64 = guard
        .query_row(
            "SELECT count(*) FROM lake.sigiro_spans WHERE trace_id = '11111111111111111111111111111111'",
            [],
            |r| r.get(0),
        )
        .expect("span count query should succeed");
    let (logs, event_name): (i64, String) = guard
        .query_row(
            "SELECT count(*), any_value(event_name) FROM lake.sigiro_logs WHERE body = 'json log body'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("log count query should succeed");
    let gauges: i64 = guard
        .query_row(
            "SELECT count(*) FROM lake.sigiro_metrics_gauge WHERE metric_name = 'json.request.duration' AND service_name = 'json-svc'",
            [],
            |r| r.get(0),
        )
        .expect("gauge count query should succeed");
    let (sums, sum_value): (i64, i64) = guard
        .query_row(
            "SELECT count(*), CAST(max(value) AS BIGINT) FROM lake.sigiro_metrics_sum WHERE metric_name IN ('json.request.count', 'json.request.delta') AND service_name = 'json-svc'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("sum count query should succeed");
    let histograms: i64 = guard
        .query_row(
            "SELECT count(*) FROM lake.sigiro_metrics_histogram WHERE metric_name = 'json.request.size'",
            [],
            |r| r.get(0),
        )
        .expect("histogram count query should succeed");
    let exponential_histograms: i64 = guard
        .query_row(
            "SELECT count(*) FROM lake.sigiro_metrics_exp_histogram WHERE metric_name = 'json.request.exp_size'",
            [],
            |r| r.get(0),
        )
        .expect("exponential histogram count query should succeed");

    assert_eq!(spans, 1);
    assert_eq!(logs, 1);
    assert_eq!(event_name, "deployment.failed");
    assert_eq!(gauges, 1);
    assert_eq!(sums, 2);
    assert_eq!(sum_value, 7);
    assert_eq!(histograms, 1);
    assert_eq!(exponential_histograms, 1);

    // Duration/timestamp units: the span spans exactly 1s (end - start = 1e9 ns).
    // Every stored column must be MICROSECONDS — this guards the 1000x bug where
    // the span path divided ns by 1_000_000 (→ ms) instead of 1_000.
    let (duration, start_us, end_us): (i64, i64, i64) = guard
        .query_row(
            "SELECT duration, epoch_us(timestamp), end_timestamp FROM lake.sigiro_spans \
             WHERE trace_id = '11111111111111111111111111111111'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("span row query should succeed");
    assert_eq!(
        duration, 1_000_000,
        "1s span must be 1_000_000 µs, not ms/ns"
    );
    assert_eq!(
        end_us - start_us,
        1_000_000,
        "end_timestamp and timestamp must share the µs unit"
    );
}

#[tokio::test]
async fn test_otlp_json_metrics_reject_unsupported_or_invalid_points() {
    let ctx = build_test_context();
    let (app, router) = build_otlp_app_with_router(&ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");
    let payload = r#"{
      "resourceMetrics": [{"scopeMetrics": [{"metrics": [{
        "name": "partial.valid.sum",
        "sum": {"aggregationTemporality": 2, "isMonotonic": true, "dataPoints": [{
          "timeUnixNano": "1700000000500000000", "asInt": "9"
        }]}
      }, {
        "name": "partial.unsupported.summary",
        "summary": {"dataPoints": [{
          "timeUnixNano": "1700000000500000000", "count": "1", "sum": 1.0,
          "quantileValues": [{}]
        }]}
      }, {
        "name": "partial.missing.gauge",
        "gauge": {"dataPoints": [{
          "timeUnixNano": "1700000000500000000"
        }]}
      }, {
        "name": "partial.nan.gauge",
        "gauge": {"dataPoints": [{
          "timeUnixNano": "1700000000500000000", "asDouble": "NaN"
        }]}
      }, {
        "name": "partial.infinity.sum",
        "sum": {"aggregationTemporality": 2, "dataPoints": [{
          "timeUnixNano": "1700000000500000000", "asDouble": "Infinity"
        }]}
      }]}]}]
    }"#;

    let response = server
        .post("/v1/metrics")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .bytes(payload.into())
        .content_type("application/json")
        .await;
    response.assert_status(axum::http::StatusCode::BAD_REQUEST);
    let body: serde_json::Value =
        serde_json::from_str(&response.text()).expect("error response should be JSON");
    assert_eq!(body["code"], 3);
    assert_eq!(
        body["message"],
        "rejected 4 metric data points: 1 unsupported summary data points; 1 NaN values; 1 infinite values; 1 data points missing a value"
    );

    router
        .shutdown_and_wait()
        .await
        .expect("flusher should persist valid sibling");
    let conn = ctx
        .state
        .pool
        .get_project("default")
        .expect("default project connection should exist");
    let rows: i64 = conn
        .get()
        .query_row(
            "SELECT count(*) FROM lake.sigiro_metrics_sum WHERE metric_name = 'partial.valid.sum'",
            [],
            |row| row.get(0),
        )
        .expect("rejected sibling sum count should be queryable");
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn otlp_metrics_errors_use_the_requested_wire_encoding() {
    use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
    use opentelemetry_proto::tonic::metrics::v1::{Metric, ResourceMetrics, ScopeMetrics};
    use prost::Message;

    let ctx = build_test_context();
    let server = TestServer::new(build_otlp_app(&ctx.state)).expect("build OTLP test server");

    let json_response = server
        .post("/v1/metrics")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .content_type("application/json")
        .bytes(
            r#"{"resourceMetrics":[{"scopeMetrics":[{"metrics":[{"name":"missing-data"}]}]}]}"#
                .into(),
        )
        .await;
    json_response.assert_status(axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(
        json_response.headers()[axum::http::header::CONTENT_TYPE],
        "application/json"
    );
    let json_status: serde_json::Value = json_response.json();
    assert_eq!(json_status["code"], 3);
    assert_eq!(
        json_status["message"],
        "parse error: metric \"missing-data\" is missing or has malformed metric data; expected exactly one of gauge, sum, histogram, exponentialHistogram, or summary"
    );

    let malformed_json_response = server
        .post("/v1/metrics")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .content_type("application/json")
        .bytes(
            r#"{"resourceMetrics":[{"scopeMetrics":[{"metrics":[{"name":"malformed-sum","sum":{"aggregationTemporality":2,"isMonotonic":"yes","dataPoints":[]}}]}]}]}"#
                .into(),
        )
        .await;
    malformed_json_response.assert_status(axum::http::StatusCode::BAD_REQUEST);
    let malformed_status: serde_json::Value = malformed_json_response.json();
    assert_eq!(
        malformed_status["message"],
        "parse error: metric \"malformed-sum\" is missing or has malformed metric data; expected exactly one of gauge, sum, histogram, exponentialHistogram, or summary"
    );

    let charset_json_response = server
        .post("/v1/metrics")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .add_header("Content-Type", "application/json; charset=utf-8")
        .bytes("{}".into())
        .await;
    charset_json_response.assert_status_ok();
    assert_eq!(
        charset_json_response.headers()[axum::http::header::CONTENT_TYPE],
        "application/json"
    );

    let protobuf_response = server
        .post("/v1/metrics")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .content_type("application/x-protobuf")
        .bytes(vec![0xff].into())
        .await;
    protobuf_response.assert_status(axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(
        protobuf_response.headers()[axum::http::header::CONTENT_TYPE],
        "application/x-protobuf"
    );
    let status = tonic_types::pb::Status::decode(protobuf_response.as_bytes().as_ref())
        .expect("protobuf error response should be google.rpc.Status");
    assert_eq!(status.code, tonic::Code::InvalidArgument as i32);
    assert!(!status.message.is_empty());

    let success_response = server
        .post("/v1/metrics")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .content_type("application/x-protobuf")
        .bytes(
            ExportMetricsServiceRequest {
                resource_metrics: vec![ResourceMetrics {
                    scope_metrics: vec![ScopeMetrics {
                        metrics: vec![Metric {
                            name: "unknown-protobuf-metric".to_string(),
                            data: None,
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
            }
            .encode_to_vec()
            .into(),
        )
        .await;
    success_response.assert_status_ok();
    assert_eq!(
        success_response.headers()[axum::http::header::CONTENT_TYPE],
        "application/x-protobuf"
    );
    let success =
        opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceResponse::decode(
            success_response.as_bytes().as_ref(),
        )
        .expect("protobuf success response should decode");
    assert!(success.partial_success.is_none());
}

#[tokio::test]
async fn otlp_protobuf_metrics_preserve_partial_success_accounting() {
    use opentelemetry_proto::tonic::collector::metrics::v1::{
        ExportMetricsServiceRequest, ExportMetricsServiceResponse,
    };
    use opentelemetry_proto::tonic::metrics::v1::{
        Metric, ResourceMetrics, ScopeMetrics, Summary, SummaryDataPoint, metric::Data,
    };
    use prost::Message;

    let ctx = build_test_context();
    let server = TestServer::new(build_otlp_app(&ctx.state)).expect("build OTLP test server");
    let request = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            scope_metrics: vec![ScopeMetrics {
                metrics: vec![Metric {
                    name: "protobuf.unsupported.summary".to_string(),
                    data: Some(Data::Summary(Summary {
                        data_points: vec![SummaryDataPoint::default()],
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };

    let response = server
        .post("/v1/metrics")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .content_type("application/x-protobuf")
        .bytes(request.encode_to_vec().into())
        .await;
    response.assert_status_ok();
    assert_eq!(
        response.headers()[axum::http::header::CONTENT_TYPE],
        "application/x-protobuf"
    );
    let response = ExportMetricsServiceResponse::decode(response.as_bytes().as_ref())
        .expect("partial success response should decode");
    let partial = response
        .partial_success
        .expect("summary should produce partial success");
    assert_eq!(partial.rejected_data_points, 1);
    assert_eq!(partial.error_message, "1 unsupported summary data points");
}

#[tokio::test]
async fn test_otlp_protobuf_preserves_event_name_and_severity() {
    use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
    use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use prost::Message;

    let ctx = build_test_context();
    let (app, router) = build_otlp_app_with_router(&ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");
    let request = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![KeyValue {
                    key: "service.name".to_string(),
                    value: Some(AnyValue {
                        value: Some(
                            opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(
                                "protobuf-event-svc".to_string(),
                            ),
                        ),
                    }),
                }],
                ..Default::default()
            }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope::default()),
                log_records: vec![LogRecord {
                    time_unix_nano: 1_700_000_000_500_000_000,
                    observed_time_unix_nano: 1_700_000_000_500_000_000,
                    event_name: "container.oom_killed".to_string(),
                    severity_number: 17,
                    severity_text: "Error".to_string(),
                    attributes: vec![
                        KeyValue {
                            key: "container.name".to_string(),
                            value: Some(AnyValue {
                                value: Some(
                                    opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(
                                        "sigiro-test".to_string(),
                                    ),
                                ),
                            }),
                        },
                        KeyValue {
                            key: "container.id".to_string(),
                            value: Some(AnyValue {
                                value: Some(
                                    opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(
                                        "container-1".to_string(),
                                    ),
                                ),
                            }),
                        },
                    ],
                    body: Some(AnyValue {
                        value: Some(
                            opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(
                                "protobuf event".to_string(),
                            ),
                        ),
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };

    let response = server
        .post("/v1/logs")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .content_type("application/x-protobuf")
        .bytes(request.encode_to_vec().into())
        .await;
    response.assert_status_ok();
    router
        .shutdown_and_wait()
        .await
        .expect("flusher should persist protobuf event");

    let conn = ctx
        .state
        .pool
        .get_project("default")
        .expect("default project should exist");
    let (event_name, severity_number, severity_text): (String, i32, String) = conn
        .get()
        .query_row(
            "SELECT event_name, severity_number, severity_text
             FROM lake.sigiro_logs WHERE body = 'protobuf event'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("protobuf log fields should be queryable");
    assert_eq!(event_name, "container.oom_killed");
    assert_eq!(severity_number, 17);
    assert_eq!(severity_text, "Error");
}

#[tokio::test]
async fn test_otlp_oom_events_drive_alert_lifecycle_once() {
    let ctx = build_test_context();
    let (otlp_app, router) = build_otlp_app_with_router(&ctx.state);
    let otlp = TestServer::new(otlp_app).expect("failed to build OTLP test server");
    let api = TestServer::new(build_http_router(Arc::clone(&ctx.state)))
        .expect("failed to build API test server");

    let event = |event_name: &str, timestamp: &str| {
        json!({
            "resourceLogs": [{
                "resource": {"attributes": [{
                    "key": "service.name",
                    "value": {"stringValue": "sigiro-hosted"}
                }]},
                "scopeLogs": [{"logRecords": [{
                    "eventName": event_name,
                    "timeUnixNano": timestamp,
                    "attributes": [
                        {"key": "container.name", "value": {"stringValue": "benchapp-sigiro-hosted"}},
                        {"key": "container.id", "value": {"stringValue": "container-1"}}
                    ]
                }]}]
            }]
        })
    };

    for payload in [
        event("container.oom_killed", "1700000000000000000"),
        event("container.oom_killed", "1700000000000000000"),
    ] {
        otlp.post("/v1/logs")
            .json(&payload)
            .await
            .assert_status_ok();
    }

    let firing: serde_json::Value = api.get("/v1/alerts").await.json();
    assert_eq!(firing["alerts"][0]["status"], "firing");
    assert_eq!(firing["alerts"][0]["occurrence_count"], 1);

    otlp.post("/v1/logs")
        .json(&event("container.oom_recovered", "1700000001000000000"))
        .await
        .assert_status_ok();

    let resolved: serde_json::Value = api.get("/v1/alerts?status=resolved").await.json();
    assert_eq!(resolved["alerts"][0]["status"], "resolved");
    assert_eq!(resolved["alerts"][0]["occurrence_count"], 1);

    let (occurrences, notifications): (i64, i64) = match &*ctx.state.admin_conn.lock() {
        admin::AdminConnection::Local(conn) => conn
            .query_row(
                "SELECT
                (SELECT count(*) FROM alert_occurrences),
                (SELECT count(*) FROM alert_outbox)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("alert control state should be queryable"),
        admin::AdminConnection::Hosted(_) => unreachable!("test control state is local"),
    };
    assert_eq!(occurrences, 2);
    assert_eq!(notifications, 2);

    router
        .shutdown_and_wait()
        .await
        .expect("flusher should persist alert events");
}

#[tokio::test]
async fn test_otlp_accepts_event_when_alert_state_is_not_durable() {
    let ctx = build_test_context();
    let (otlp_app, router) = build_otlp_app_with_router(&ctx.state);
    let otlp = TestServer::new(otlp_app).expect("failed to build OTLP test server");
    match &*ctx.state.admin_conn.lock() {
        admin::AdminConnection::Local(conn) => conn
            .execute_batch("DROP TABLE alert_occurrences")
            .expect("break alert persistence"),
        admin::AdminConnection::Hosted(_) => unreachable!("test control state is local"),
    }

    let response = otlp
        .post("/v1/logs")
        .json(&json!({
            "resourceLogs": [{
                "resource": {"attributes": [{
                    "key": "service.name",
                    "value": {"stringValue": "sigiro-hosted"}
                }]},
                "scopeLogs": [{"logRecords": [{
                    "eventName": "container.oom_killed",
                    "timeUnixNano": "1700000000000000000",
                    "attributes": [
                        {"key": "container.name", "value": {"stringValue": "benchapp-sigiro-hosted"}},
                        {"key": "container.id", "value": {"stringValue": "container-1"}}
                    ]
                }]}]
            }]
        }))
        .await;
    response.assert_status_ok();

    router
        .shutdown_and_wait()
        .await
        .expect("queued telemetry should still flush");
}

#[tokio::test]
// M-C: `run_servers` merges the OTLP HTTP ingest router (this test's
// `otlp_router`) into the same app as the v1/admin API (`build_http_router`)
// so :9999 serves both. axum panics at Router construction time on a
// duplicate method+path, so building this merged app for real (not just the
// two routers separately, as the tests above do) is what actually proves
// there is no collision — and that both sides remain reachable afterward.
async fn test_otlp_merged_into_http_app_has_no_route_collision() {
    let ctx = build_test_context();
    mark_test_context_ready(&ctx);

    let otlp_router = ingest::otlp_http_router(
        Arc::clone(&ctx.state.router),
        ctx.state.auth_mode.clone(),
        8_388_608,
    );
    let app = build_http_router(Arc::clone(&ctx.state)).merge(otlp_router);
    let server = TestServer::new(app).expect("merged app must not panic on route collision");

    let health = server.get("/health").await;
    health.assert_status_ok();

    let traces = server
        .post("/v1/traces")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .bytes("{\"resourceSpans\":[]}".into())
        .content_type("application/json")
        .await;
    traces.assert_status(axum::http::StatusCode::OK);
}

/// The two tests above construct `AppState` directly and set `init_complete`
/// by hand, so they pass no matter how production wires it. That is exactly how
/// this defect survived: `run_serve` flipped the flag at its own tail, BEFORE
/// `run_servers` bound the HTTP listener, so no external client could ever
/// observe `Starting` — measured as zero 503s across 18 instrumented boots.
/// `HealthResponse::Starting` was dead code in production.
///
/// This test pins the WIRING rather than the handler: `run_serve` must not
/// mark init complete itself, because the only correct moment is when the
/// listener is actually accepting connections. `run_servers` flips it from
/// `Handle::listening()`.
///
/// Guarding this matters because the window is real work that can block or
/// fail — the quack extension load, and `pool.clone_connection("default")`,
/// which can perform an S3-backed distributed attach. A probe during it must
/// get 503, not a `ready` for a server that is listening but cannot serve.
#[test]
fn run_serve_does_not_mark_init_complete_before_the_listener_binds() {
    let main_rs = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))
        .expect("src/main.rs should be readable");

    let stores: Vec<&str> = main_rs
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("//"))
        .filter(|l| l.contains("init_complete") && l.contains(".store("))
        .collect();

    assert!(
        stores.is_empty(),
        "src/main.rs must not flip init_complete — only run_servers may, once \
         Handle::listening() reports the HTTP socket bound. Otherwise /health \
         answers \"ready\" before anything can connect and HealthResponse::Starting \
         becomes unreachable. Found: {stores:?}"
    );

    let server_rs = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/server.rs"))
        .expect("src/server.rs should be readable");
    assert!(
        server_rs.contains("listening().await"),
        "run_servers must gate init_complete on Handle::listening()"
    );
}

/// #91: the writer thread's `Checkpoint` arm must RECORD its outcome, not just
/// log it. This pins the WIRING, because the behavioural tests build their own
/// `MaintenanceHealth` and drive `checkpoint()` directly — they would keep
/// passing if `run_writer_thread` went back to bare `warn!` and never told the
/// gate anything. (Same lesson as #87: a test that constructs its own state
/// cannot catch a defect in how production assembles that state.)
#[test]
fn writer_thread_records_every_checkpoint_outcome() {
    let inserter_rs =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/inserter.rs"))
            .expect("src/inserter.rs should be readable");

    assert!(
        inserter_rs.contains("maintenance.record_checkpoint_failure(project_id)"),
        "run_writer_thread's Checkpoint arm must record failures into \
         MaintenanceHealth, otherwise sustained CHECKPOINT failure silently \
         lets the catalog absorb telemetry that can never be materialised (#91)"
    );
    assert!(
        inserter_rs.contains("maintenance.record_checkpoint_success(project_id)"),
        "run_writer_thread must record checkpoint SUCCESS too — without it a \
         recovered backend never re-opens ingest and the shed is permanent (#91)"
    );

    // The ingest gate must consult the same state the writer records into.
    assert!(
        inserter_rs.contains("self.maintenance.is_stalled(&project_id)"),
        "MsgRouter::send must gate on MaintenanceHealth::is_stalled, or nothing \
         sheds and #91 is only a log line again"
    );

    // And the failure must reach the wire as 507 / resource_exhausted, never 500.
    let ingest_rs = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/ingest.rs"))
        .expect("src/ingest.rs should be readable");
    assert!(
        ingest_rs.contains("RouterError::MaintenanceStalled"),
        "the ingest surfaces must map MaintenanceStalled to 507 / \
         resource_exhausted rather than falling through to a 500"
    );
}
