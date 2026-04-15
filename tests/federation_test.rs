use axum::Router;
use axum::middleware as axum_mw;
use axum::routing::{delete, get, post};
use rusqlite::Connection;
use sigiro::admin;
use sigiro::auth;
use sigiro::federation;
use sigiro::pool::ProjectPool;
use sigiro::server::AppState;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tower::ServiceExt;

/// Build a minimal Axum app that mirrors the federation routes from `run_servers`.
fn build_app(state: Arc<AppState>) -> Router {
    let v1 = Router::new()
        .route("/federation/workers", post(register_worker_handler))
        .route("/federation/workers", get(list_workers_handler))
        .route(
            "/federation/workers/{id}",
            delete(deregister_worker_handler),
        )
        .layer(axum_mw::from_fn_with_state(
            state.clone(),
            auth::auth_middleware,
        ));

    Router::new().nest("/v1", v1).with_state(state)
}

async fn register_worker_handler(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    axum::Extension(_auth): axum::Extension<auth::AuthContext>,
    axum::Json(worker): axum::Json<federation::WorkerInfo>,
) -> Result<axum::Json<serde_json::Value>, axum_anyhow::ApiError> {
    let id = worker.id.clone();
    let guard = state
        .admin_conn
        .lock()
        .map_err(|e| anyhow::anyhow!("admin DB mutex: {e}"))?;
    federation::register_worker(&guard, &worker)
        .map_err(|e| axum_anyhow::internal_error("Federation", &e.to_string()))?;
    Ok(axum::Json(serde_json::json!({
        "registered": true,
        "id": id,
    })))
}

async fn deregister_worker_handler(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    axum::Extension(_auth): axum::Extension<auth::AuthContext>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<axum::Json<serde_json::Value>, axum_anyhow::ApiError> {
    let guard = state
        .admin_conn
        .lock()
        .map_err(|e| anyhow::anyhow!("admin DB mutex: {e}"))?;
    let deregistered = federation::deregister_worker(&guard, &id)
        .map_err(|e| axum_anyhow::internal_error("Federation", &e.to_string()))?;
    if deregistered {
        Ok(axum::Json(serde_json::json!({ "deregistered": true })))
    } else {
        Err(axum_anyhow::not_found(
            "Worker Not Found",
            &format!("No worker found with id: {id}"),
        ))
    }
}

async fn list_workers_handler(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    axum::Extension(_auth): axum::Extension<auth::AuthContext>,
) -> Result<axum::Json<serde_json::Value>, axum_anyhow::ApiError> {
    let guard = state
        .admin_conn
        .lock()
        .map_err(|e| anyhow::anyhow!("admin DB mutex: {e}"))?;
    let workers = federation::list_workers(&guard)
        .map_err(|e| axum_anyhow::internal_error("Federation", &e.to_string()))?;
    Ok(axum::Json(serde_json::json!({ "workers": workers })))
}

/// Set up a fresh test environment with its own temp dir and admin DB.
///
/// All tests must run through [`federation_endpoints_e2e`] to avoid races
/// on the `SIGIRO_DATA_DIR` env var (shared across parallel test threads).
fn setup() -> (TempDir, Arc<Mutex<Connection>>) {
    let tmp = TempDir::new().expect("temp dir");
    // SAFETY: we only call this from the single serial e2e test.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("SIGIRO_DATA_DIR", tmp.path());
    }

    let admin_conn = Connection::open_in_memory().expect("in-memory admin db");
    admin::create_keys_table(&admin_conn).expect("keys table");
    admin::create_instance_status_table(&admin_conn).expect("status table");
    federation::create_workers_table(&admin_conn).expect("workers table");

    (tmp, Arc::new(Mutex::new(admin_conn)))
}

fn make_state(admin_conn: &Arc<Mutex<Connection>>) -> Arc<AppState> {
    let data_dir =
        std::path::PathBuf::from(std::env::var("SIGIRO_DATA_DIR").expect("data dir set"));
    Arc::new(AppState {
        pool: Arc::new(ProjectPool::new(data_dir, 16)),
        admin_conn: Arc::clone(admin_conn),
        http_client: reqwest::Client::new(),
    })
}

fn test_worker(id: &str) -> federation::WorkerInfo {
    federation::WorkerInfo {
        id: id.to_string(),
        endpoint: "http://localhost:9999".to_string(),
        catalog_path: "/data/default/catalog.sqlite".to_string(),
        mode: "otlp".to_string(),
        platform: federation::PlatformInfo {
            os: "linux".to_string(),
            kernel: "5.15.0".to_string(),
            arch: "x86_64".to_string(),
            container: "docker".to_string(),
        },
        ebpf_status: "ready".to_string(),
        capabilities: federation::WorkerCapabilities {
            bpf: true,
            net_admin: true,
        },
        missing_permissions: vec![],
        suggestions: vec![],
    }
}

#[tokio::test]
async fn federation_endpoints_e2e() {
    let (_tmp, admin_conn) = setup();

    // --- 1. Unauthenticated request returns 401 ---
    let app = build_app(make_state(&admin_conn));
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/v1/federation/workers")
                .body(axum::body::Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request");
    assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);

    // --- 2. Register a worker via POST /v1/federation/workers ---
    let app = build_app(make_state(&admin_conn));
    let worker = test_worker("worker-1");
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/federation/workers")
                .header("authorization", "Bearer sigiro_dev_test")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::to_string(&worker).expect("serialize worker"),
                ))
                .expect("build request"),
        )
        .await
        .expect("request");

    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1024)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("parse json");
    assert_eq!(json["registered"], true);
    assert_eq!(json["id"], "worker-1");

    // --- 3. List workers — verify the registered worker appears ---
    let app = build_app(make_state(&admin_conn));
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/v1/federation/workers")
                .header("authorization", "Bearer sigiro_dev_test")
                .body(axum::body::Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request");

    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1024)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("parse json");
    let workers = json["workers"].as_array().expect("workers array");
    assert_eq!(workers.len(), 1);
    assert_eq!(workers[0]["id"], "worker-1");
    assert_eq!(workers[0]["endpoint"], "http://localhost:9999");
    assert_eq!(workers[0]["catalog_path"], "/data/default/catalog.sqlite");
    assert_eq!(workers[0]["platform"]["os"], "linux");
    assert_eq!(workers[0]["capabilities"]["bpf"], true);

    // --- 4. Deregister the worker via DELETE /v1/federation/workers/{id} ---
    let app = build_app(make_state(&admin_conn));
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("DELETE")
                .uri("/v1/federation/workers/worker-1")
                .header("authorization", "Bearer sigiro_dev_test")
                .body(axum::body::Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request");

    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1024)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("parse json");
    assert_eq!(json["deregistered"], true);

    // --- 5. Verify the worker is gone from the list ---
    let app = build_app(make_state(&admin_conn));
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/v1/federation/workers")
                .header("authorization", "Bearer sigiro_dev_test")
                .body(axum::body::Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request");

    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1024)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("parse json");
    let workers = json["workers"].as_array().expect("workers array");
    assert!(
        workers.is_empty(),
        "workers should be empty after deregister"
    );

    // --- 6. Deregister non-existent worker returns 404 ---
    let app = build_app(make_state(&admin_conn));
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("DELETE")
                .uri("/v1/federation/workers/ghost-worker")
                .header("authorization", "Bearer sigiro_dev_test")
                .body(axum::body::Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request");

    assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
}
