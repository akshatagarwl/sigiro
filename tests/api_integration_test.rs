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
}

/// Build a test AppState with temp directories.
/// Sets SIGIRO_EXTENSION_DIR to a stable path so all tests share one
/// extension cache (downloads happen once, not per-test), and creates
/// an in-memory SQLite admin DB with all required tables.
fn build_test_context() -> TestContext {
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

    let pool = Arc::new(ProjectPool::new(data_dir.clone(), 4));

    // Explicit DI: initialize the default project connection before handlers access it.
    pool.create_project("default")
        .expect("failed to init default project connection");

    // In-memory SQLite admin DB with all required tables
    let admin_conn = Connection::open_in_memory().expect("failed to open in-memory SQLite");
    admin::create_instance_status_table(&admin_conn)
        .expect("failed to create instance_status table");
    sigiro::federation::create_workers_table(&admin_conn).expect("failed to create workers table");

    let admin_conn = Arc::new(Mutex::new(admin_conn));

    let router = Arc::new(sigiro::inserter::MsgRouter::new(Arc::clone(&pool)));
    router.start();
    let state = Arc::new(AppState {
        pool,
        admin_conn,
        http_client: reqwest::Client::new(),
        router,
        auth_mode: sigiro::auth::AuthMode::Key {
            key: "sigiro_dev_test".to_string(),
        },
        data_dir: data_dir.clone(),
        init_complete: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });

    TestContext { state }
}

/// Build the OTLP HTTP router (port 4318).
fn build_otlp_app(state: &Arc<AppState>) -> axum::Router {
    let router = Arc::new(MsgRouter::new(Arc::clone(&state.pool)));
    router.start();
    ingest::otlp_http_router(router, state.auth_mode.clone())
}

// -- Tests ----------------------------------------------------------------

/// Mark the test AppState as "init complete" by flipping the
/// `init_complete` AtomicBool that the production server flips at the tail
/// of init (see src/main.rs). Health checks must reflect that state.
fn mark_test_context_ready(ctx: &TestContext) {
    ctx.state.mark_init_complete();
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
    // federation setup, etc.). Health probes must see this as not ready.
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
async fn test_query_without_auth_returns_401() {
    let ctx = build_test_context();
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    let resp = server.post("/v1/query").text("SELECT 1").await;
    resp.assert_status(axum::http::StatusCode::UNAUTHORIZED);
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
async fn test_admin_status_returns_200() {
    let ctx = build_test_context();
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    let resp = server.get("/admin/status").await;
    resp.assert_status_ok();

    let body: serde_json::Value = serde_json::from_str(&resp.text()).expect("valid JSON");
    // Empty status is a valid JSON object
    assert!(body.is_object(), "expected JSON object");
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
