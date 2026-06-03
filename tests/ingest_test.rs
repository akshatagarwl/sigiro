#[test]
fn grpc_auth_open_mode_allows_any() {
    let mut metadata = tonic::metadata::MetadataMap::new();
    metadata.insert("authorization", "Bearer anything".parse().unwrap());
    let project_id = sigiro::ingest::extract_project_id_grpc("", &metadata).unwrap();
    assert_eq!(project_id, "default");
}

#[test]
fn grpc_auth_missing_header_fails() {
    let metadata = tonic::metadata::MetadataMap::new();
    let err = sigiro::ingest::extract_project_id_grpc("secret", &metadata).unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
}

#[test]
fn grpc_auth_invalid_key_fails() {
    let mut metadata = tonic::metadata::MetadataMap::new();
    metadata.insert("authorization", "Bearer wrong_key".parse().unwrap());
    let err = sigiro::ingest::extract_project_id_grpc("secret", &metadata).unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
}

#[test]
fn grpc_auth_matching_key_succeeds() {
    let mut metadata = tonic::metadata::MetadataMap::new();
    metadata.insert("authorization", "Bearer my_secret".parse().unwrap());
    let project_id = sigiro::ingest::extract_project_id_grpc("my_secret", &metadata).unwrap();
    assert_eq!(project_id, "default");
}

#[test]
fn http_auth_open_mode_allows_any() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", "Bearer anything".parse().unwrap());
    let project_id = sigiro::ingest::extract_project_id_http("", &headers).unwrap();
    assert_eq!(project_id, "default");
}

#[test]
fn http_auth_missing_header_returns_401() {
    let headers = axum::http::HeaderMap::new();
    let err = sigiro::ingest::extract_project_id_http("secret", &headers).unwrap_err();
    assert_eq!(err, axum::http::StatusCode::UNAUTHORIZED);
}

#[test]
fn http_auth_invalid_key_returns_401() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", "Bearer wrong_key".parse().unwrap());
    let err = sigiro::ingest::extract_project_id_http("secret", &headers).unwrap_err();
    assert_eq!(err, axum::http::StatusCode::UNAUTHORIZED);
}

#[test]
fn http_auth_matching_key_succeeds() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", "Bearer my_secret".parse().unwrap());
    let project_id = sigiro::ingest::extract_project_id_http("my_secret", &headers).unwrap();
    assert_eq!(project_id, "default");
}
