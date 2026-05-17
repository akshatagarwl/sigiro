use parking_lot::Mutex;
use rusqlite::Connection;

fn test_admin_conn() -> Mutex<Connection> {
    let conn = Connection::open_in_memory().unwrap();
    sigiro::admin::create_keys_table(&conn).unwrap();
    Mutex::new(conn)
}

#[test]
fn grpc_auth_dev_key_returns_dev_project() {
    let admin = test_admin_conn();
    let mut metadata = tonic::metadata::MetadataMap::new();
    metadata.insert(
        "authorization",
        "Bearer sigiro_dev_test123".parse().unwrap(),
    );
    let project_id = sigiro::ingest::extract_project_id_grpc(&admin, &metadata).unwrap();
    assert_eq!(project_id, "dev_project");
}

#[test]
fn grpc_auth_missing_header_fails() {
    let admin = test_admin_conn();
    let metadata = tonic::metadata::MetadataMap::new();
    let err = sigiro::ingest::extract_project_id_grpc(&admin, &metadata).unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
}

#[test]
fn grpc_auth_invalid_key_fails() {
    let admin = test_admin_conn();
    let mut metadata = tonic::metadata::MetadataMap::new();
    metadata.insert(
        "authorization",
        "Bearer sigiro_FAKEFAKE_FAKEFAKEFAKEFAKEFAKEFAKE"
            .parse()
            .unwrap(),
    );
    let err = sigiro::ingest::extract_project_id_grpc(&admin, &metadata).unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
}

#[test]
fn grpc_auth_real_key_returns_project_id() {
    let admin = test_admin_conn();
    let raw_key = sigiro::admin::create_key(&admin, "my_project", "free", 100).unwrap();
    let mut metadata = tonic::metadata::MetadataMap::new();
    metadata.insert(
        "authorization",
        format!("Bearer {raw_key}").parse().unwrap(),
    );
    let project_id = sigiro::ingest::extract_project_id_grpc(&admin, &metadata).unwrap();
    assert_eq!(project_id, "my_project");
}

#[test]
fn http_auth_dev_key_returns_dev_project() {
    let admin = test_admin_conn();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "authorization",
        "Bearer sigiro_dev_test123".parse().unwrap(),
    );
    let project_id = sigiro::ingest::extract_project_id_http(&admin, &headers).unwrap();
    assert_eq!(project_id, "dev_project");
}

#[test]
fn http_auth_missing_header_returns_401() {
    let admin = test_admin_conn();
    let headers = axum::http::HeaderMap::new();
    let err = sigiro::ingest::extract_project_id_http(&admin, &headers).unwrap_err();
    assert_eq!(err, axum::http::StatusCode::UNAUTHORIZED);
}

#[test]
fn http_auth_invalid_key_returns_401() {
    let admin = test_admin_conn();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "authorization",
        "Bearer sigiro_FAKEFAKE_FAKEFAKEFAKEFAKEFAKEFAKE"
            .parse()
            .unwrap(),
    );
    let err = sigiro::ingest::extract_project_id_http(&admin, &headers).unwrap_err();
    assert_eq!(err, axum::http::StatusCode::UNAUTHORIZED);
}

#[test]
fn http_auth_real_key_returns_project_id() {
    let admin = test_admin_conn();
    let raw_key = sigiro::admin::create_key(&admin, "http_proj", "builder", 500).unwrap();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {raw_key}").parse().unwrap(),
    );
    let project_id = sigiro::ingest::extract_project_id_http(&admin, &headers).unwrap();
    assert_eq!(project_id, "http_proj");
}
