//! Integration tests for sigiro HTTP API endpoints.
//!
//! Uses axum-test to exercise the Router directly (no TCP listener).
//! Test state uses temp directories for SIGIRO_DATA_DIR and an in-memory
//! SQLite admin DB.

#![allow(clippy::disallowed_methods)]

use axum_test::TestServer;
use serde_json::json;

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

    // Prewarm exactly as `sigiro fetch-extensions` does, instead of relying on
    // autoload to populate the cache incidentally.
    //
    // Without this the suite depended on an accident: `ducklake` and `httpfs`
    // pull `parquet` in transitively, but nothing pulls `json` or `icu`, so on a
    // platform where those are not statically linked (macOS — see
    // `storage::extensions_needing_install`) every test here failed at
    // `create_project` with a missing `json.duckdb_extension` as soon as
    // /tmp/sigiro-test-extensions was cleared. That is the same defect users hit
    // on a fresh install, and the shared cache is what hid it locally.
    //
    // Idempotent and cheap once the cache is warm, and a no-op on a build where
    // the extensions are static. The argument is ignored in practice because
    // `resolve_extension_dir` prefers SIGIRO_EXTENSION_DIR, set just above — the
    // same resolution serve-time uses, so this warms the directory the pool
    // actually reads.
    sigiro::storage::prewarm_extensions(&data_dir)
        .expect("failed to prewarm DuckDB extensions for the test extension cache");

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
    let server = TestServer::new(app).expect("failed to build test server");

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
            "POST /admin/tenants",
            server
                .post("/admin/tenants")
                .json(&json!({"id": "t"}))
                .await,
        ),
        (
            "POST /admin/tenants (wrong token)",
            server
                .post("/admin/tenants")
                .add_header("Authorization", bad)
                .json(&json!({"id": "t"}))
                .await,
        ),
        (
            "POST /admin/keys",
            server
                .post("/admin/keys")
                .json(&json!({"tenant_id": "t"}))
                .await,
        ),
        (
            "POST /admin/keys (wrong token)",
            server
                .post("/admin/keys")
                .add_header("Authorization", bad)
                .json(&json!({"tenant_id": "t"}))
                .await,
        ),
        (
            "DELETE /admin/keys/{short_token}",
            server.delete("/admin/keys/abcd1234").await,
        ),
        (
            "DELETE /admin/keys/{short_token} (wrong token)",
            server
                .delete("/admin/keys/abcd1234")
                .add_header("Authorization", bad)
                .await,
        ),
        // Garbage in the header itself must not fall through either.
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

    assert_eq!(spans, 1);
    assert_eq!(logs, 1);
    assert_eq!(event_name, "deployment.failed");
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
async fn test_otlp_protobuf_preserves_event_name() {
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
                    severity_text: "ERROR".to_string(),
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
    let event_name: String = conn
        .get()
        .query_row(
            "SELECT event_name FROM lake.sigiro_logs WHERE body = 'protobuf event'",
            [],
            |row| row.get(0),
        )
        .expect("event name should be queryable");
    assert_eq!(event_name, "container.oom_killed");
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

    let (occurrences, notifications): (i64, i64) = ctx
        .state
        .admin_conn
        .lock()
        .query_row(
            "SELECT
                (SELECT count(*) FROM alert_occurrences),
                (SELECT count(*) FROM alert_outbox)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("alert control state should be queryable");
    assert_eq!(occurrences, 2);
    assert_eq!(notifications, 2);

    router
        .shutdown_and_wait()
        .await
        .expect("flusher should persist alert events");
}

#[tokio::test]
async fn test_otlp_rejects_event_when_alert_state_is_not_durable() {
    let ctx = build_test_context();
    let (otlp_app, router) = build_otlp_app_with_router(&ctx.state);
    let otlp = TestServer::new(otlp_app).expect("failed to build OTLP test server");
    ctx.state
        .admin_conn
        .lock()
        .execute_batch("DROP TABLE alert_occurrences")
        .expect("break alert persistence");

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
    response.assert_status_internal_server_error();

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
