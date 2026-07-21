//! Integration tests for sigiro HTTP API endpoints.
//!
//! Uses axum-test to exercise the Router directly (no TCP listener).
//! Test state uses temp directories for SIGIRO_DATA_DIR and an in-memory
//! SQLite admin DB.

#![allow(clippy::disallowed_methods)]

use axum_test::TestServer;

use parking_lot::Mutex;
use rusqlite::Connection;
use sigiro::admin;
use sigiro::ingest;
use sigiro::inserter::MsgRouter;
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

/// Build a test AppState with temp directories.
/// Sets SIGIRO_EXTENSION_DIR to a stable path so all tests share one
/// extension cache (downloads happen once, not per-test), and creates
/// an in-memory SQLite admin DB with all required tables.
fn build_test_context() -> TestContext {
    // Self-host default: `Open` auth (no per-request key). The auth-rejection
    // test uses `build_context_with_auth` to exercise `TenantKeys`.
    build_context_with_auth(sigiro::auth::AuthMode::Open)
}

fn build_context_with_auth(auth_mode: sigiro::auth::AuthMode) -> TestContext {
    let tmp_dir = TempDir::new().expect("failed to create temp dir");
    let data_dir = camino::Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).unwrap();

    // `pool::open_connection` reads SIGIRO_EXTENSION_DIR via
    // `crate::config::EXTENSION_DIR.value()` to locate the shared DuckDB
    // extension cache. This env var is process-global and therefore racy
    // across parallel tests, but it is read-only after the first
    // download — the worst case is two tests downloading the same file,
    // never a correctness bug. SIGIRO_DATA_DIR is NOT set here: data_dir
    // is passed explicitly through `AppState` so the env var is no
    // longer needed for any test code path.
    //
    // SAFETY: set_var is unsafe in concurrent programs; this is a test
    // helper that runs in the same process as the server under test. The
    // race is benign for the read-only cache (see comment above).
    unsafe {
        std::env::set_var("SIGIRO_EXTENSION_DIR", "/tmp/sigiro-test-extensions");
    }

    let pool = Arc::new(ProjectPool::new(
        data_dir.clone(),
        4,
        sigiro::storage::StorageMode::LocalFile,
    ));

    // Explicit DI: initialize the default project connection before handlers access it.
    pool.create_project("default")
        .expect("failed to init default project connection");

    // In-memory SQLite admin DB with all required tables
    let admin_conn = Connection::open_in_memory().expect("failed to open in-memory SQLite");
    admin::create_instance_status_table(&admin_conn)
        .expect("failed to create instance_status table");

    let admin_conn = Arc::new(Mutex::new(admin_conn));

    let router = Arc::new(sigiro::inserter::MsgRouter::new(Arc::clone(&pool)));
    router.start();
    let state = Arc::new(AppState {
        pool,
        admin_conn,
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
    let router = Arc::new(MsgRouter::new(Arc::clone(&state.pool)));
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
async fn test_openapi_documents_investigate_for_agents() {
    // Cold-start agentic discoverability: /openapi.json is the only public
    // discovery surface, so the flagship /v1/investigate MUST be fully
    // documented there — request schema, a description, and the bearer scheme.
    // This regressed silently once (schemars derive instead of utoipa ToSchema,
    // empty path macro), which is what made sigiro un-agentic from a cold URL.
    let ctx = build_test_context();
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    let spec: serde_json::Value =
        serde_json::from_str(&server.get("/openapi.json").await.text()).expect("openapi is JSON");

    let inv = &spec["paths"]["/v1/investigate"]["post"];
    assert!(
        inv["requestBody"].is_object(),
        "investigate must publish a request body schema so an agent can learn to call it"
    );
    assert!(
        inv["description"]
            .as_str()
            .unwrap_or("")
            .contains("microsecond"),
        "investigate description must state the microsecond timestamp unit"
    );
    // The response body schema graph must resolve (findings/drill fields).
    assert!(
        spec["components"]["schemas"]["InvestigateResponse"].is_object()
            && spec["components"]["schemas"]["Finding"].is_object(),
        "response schema graph must be published"
    );
    assert!(
        spec["components"]["securitySchemes"]["bearer"].is_object(),
        "bearer security scheme must be declared so an agent knows auth"
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
async fn test_query_without_valid_tenant_key_returns_401() {
    // Under `TenantKeys` (hosted) auth, a `/v1/query` with no Authorization
    // header — or a bearer token that resolves to no active tenant — must be
    // rejected at the auth middleware with 401. (Self-host `Open` mode, the
    // default for the other tests here, requires no token; the positive
    // resolve-a-real-key path is covered end-to-end by control_plane_e2e_test.)
    let control = Arc::new(sigiro::auth::ControlPlane::open_in_memory_for_tests().unwrap());
    control.create_tenant("acme").unwrap();
    let ctx = build_context_with_auth(sigiro::auth::AuthMode::TenantKeys { control });
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    // Missing Authorization header.
    let resp = server.post("/v1/query").text("SELECT 1").await;
    resp.assert_status(axum::http::StatusCode::UNAUTHORIZED);

    // Bearer token that resolves to no active tenant.
    let resp = server
        .post("/v1/query")
        .add_header("Authorization", "Bearer sk_acme_deadbeef")
        .text("SELECT 1")
        .await;
    resp.assert_status(axum::http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_new_hosted_tenant_can_query_before_first_ingest() {
    // A Machine restart preserves the hosted control plane but loses its
    // in-memory project pool. Query must therefore initialize the authenticated
    // tenant locally; provisioning-time initialization on another process is
    // insufficient, and ingest-then-query masks this lifecycle.
    let control = Arc::new(sigiro::auth::ControlPlane::open_in_memory_for_tests().unwrap());
    control.create_tenant("empty_tenant").unwrap();
    let key = control
        .mint_key("empty_tenant")
        .expect("mint key for provisioned tenant")
        .key;
    let ctx = build_context_with_auth(sigiro::auth::AuthMode::TenantKeys {
        control: Arc::clone(&control),
    });
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    let response = server
        .post("/v1/query")
        .add_header("Authorization", format!("Bearer {key}"))
        .text("SELECT count(*) AS n FROM lake.sigiro_spans")
        .await;
    response.assert_status_ok();
    let rows: serde_json::Value =
        serde_json::from_str(&response.text()).expect("query response should be JSON");
    assert_eq!(
        rows.pointer("/0/n").and_then(serde_json::Value::as_i64),
        Some(0)
    );
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
async fn test_services_with_dev_auth_returns_200() {
    let ctx = build_test_context();
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    let resp = server
        .get("/v1/services")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .await;
    resp.assert_status_ok();
    // Should return {"services":[]} for empty project
    let body: serde_json::Value = serde_json::from_str(&resp.text()).expect("valid JSON");
    assert!(
        body.get("services").is_some(),
        "expected 'services' key in response"
    );
}

#[tokio::test]
async fn test_anomalies_endpoint_returns_persisted_rows() {
    let ctx = build_test_context();

    // Seed one anomaly row into the default project's table — /v1/anomalies
    // reads exactly what the scheduled pass (sigiro::anomaly) writes.
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
                     'changepoint', 'worsening', now()::TIMESTAMP, 0.0, 42.0,
                     'SELECT * FROM lake.sigiro_spans')",
        )
        .expect("seed anomaly row");
    }

    let app = build_http_router(Arc::clone(&ctx.state));
    let server = TestServer::new(app).expect("failed to build test server");

    // Unfiltered: the seeded row comes back with the full typed shape.
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
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row["service_name"], "checkout");
    assert_eq!(row["operation"], "POST /pay");
    assert_eq!(row["signal"], "error_rate");
    assert_eq!(row["kind"], "changepoint");
    assert_eq!(row["direction"], "worsening");
    assert_eq!(row["before"], 0.0);
    assert_eq!(row["after"], 42.0);
    assert!(row["drill_down_sql"].is_string());
    assert!(row["ts_shift"].is_string(), "ISO timestamp string");

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
    assert_eq!(body.as_array().map(Vec::len), Some(1));
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
    let logs: i64 = guard
        .query_row(
            "SELECT count(*) FROM lake.sigiro_logs WHERE body = 'json log body'",
            [],
            |r| r.get(0),
        )
        .expect("log count query should succeed");
    let gauges: i64 = guard
        .query_row(
            "SELECT count(*) FROM lake.sigiro_metrics_gauge WHERE metric_name = 'json.request.duration' AND service_name = 'json-svc'",
            [],
            |r| r.get(0),
        )
        .expect("gauge count query should succeed");

    assert_eq!(spans, 1);
    assert_eq!(logs, 1);
    assert_eq!(gauges, 1);

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
