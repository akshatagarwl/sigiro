//! Integration tests for sigiro HTTP API endpoints.
//!
//! Uses axum-test to exercise the Router directly (no TCP listener).
//! Test state uses temp directories for SIGIRO_DATA_DIR and an in-memory
//! SQLite admin DB.

#![allow(clippy::disallowed_methods)]

use axum_test::TestServer;

use rusqlite::Connection;
use sigiro::admin;
use sigiro::ingest;
use sigiro::inserter::MsgRouter;
use sigiro::pool::ProjectPool;
use sigiro::server::{AppState, build_http_router};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

struct TestContext {
    _tmp_dir: TempDir,
    state: Arc<AppState>,
}

/// Build a test AppState with temp directories.
/// Sets SIGIRO_DATA_DIR to a temp dir for DuckDB and creates
/// an in-memory SQLite admin DB with all required tables.
fn build_test_context() -> TestContext {
    let tmp_dir = TempDir::new().expect("failed to create temp dir");
    let data_dir = tmp_dir.path().to_path_buf();

    // Set SIGIRO_DATA_DIR so crate::data_dir() resolves correctly.
    // This is needed by pool::create_project -> storage::init_default
    // and federation endpoints that call crate::data_dir().
    // SAFETY: This is a single-threaded test setup; no concurrent access
    // to this env var from other threads during this call.
    unsafe {
        std::env::set_var("SIGIRO_DATA_DIR", &data_dir);
        std::env::set_var("SIGIRO_EXTENSION_DIR", "/tmp/sigiro-test-extensions");
    }

    let pool = Arc::new(ProjectPool::new(data_dir, 4));

    // Explicit DI: initialize the dev_project connection before handlers access it.
    // This mirrors production behavior where ingest path calls create_project first.
    pool.create_project("dev_project")
        .expect("failed to init dev_project connection");

    // In-memory SQLite admin DB with all required tables
    let admin_conn = Connection::open_in_memory().expect("failed to open in-memory SQLite");
    admin::create_keys_table(&admin_conn).expect("failed to create keys table");
    admin::create_instance_status_table(&admin_conn)
        .expect("failed to create instance_status table");
    sigiro::federation::create_workers_table(&admin_conn).expect("failed to create workers table");

    let admin_conn = Arc::new(Mutex::new(admin_conn));

    let state = Arc::new(AppState {
        pool,
        admin_conn,
        http_client: reqwest::Client::new(),
    });

    TestContext {
        _tmp_dir: tmp_dir,
        state,
    }
}

/// Build the OTLP HTTP router (port 4318).
fn build_otlp_app(state: &Arc<AppState>) -> axum::Router {
    let router = Arc::new(MsgRouter::new(Arc::clone(&state.pool)));
    ingest::otlp_http_router(router, Arc::clone(&state.admin_conn))
}

// -- Tests ----------------------------------------------------------------

#[tokio::test]
async fn test_health_returns_ok() {
    let ctx = build_test_context();
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    let resp = server.get("/health").await;
    resp.assert_status_ok();
    resp.assert_text("ok");
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
async fn test_admin_create_key() {
    let ctx = build_test_context();
    let app = build_http_router(ctx.state);
    let server = TestServer::new(app).expect("failed to build test server");

    let body = serde_json::json!({
        "project": "test_project",
        "plan": "free",
        "rate_limit": 100
    });

    let resp = server.post("/admin/keys").json(&body).await;
    resp.assert_status_ok();

    let result: serde_json::Value = serde_json::from_str(&resp.text()).expect("valid JSON");
    let key = result
        .get("key")
        .expect("expected 'key' in response")
        .as_str()
        .expect("key should be a string");
    assert!(
        key.starts_with("sigiro_"),
        "key should start with 'sigiro_' prefix, got: {key}"
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
async fn test_otlp_traces_empty_payload_returns_400() {
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
    resp.assert_status(axum::http::StatusCode::BAD_REQUEST);
}

