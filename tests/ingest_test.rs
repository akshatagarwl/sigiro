use sigiro::auth::AuthMode;

#[tokio::test]
async fn grpc_auth_open_mode_allows_any() {
    let mut metadata = tonic::metadata::MetadataMap::new();
    metadata.insert("authorization", "Bearer anything".parse().unwrap());
    let project_id = sigiro::ingest::extract_project_id_grpc(&AuthMode::Open, &metadata)
        .await
        .unwrap();
    assert_eq!(project_id, "default");
}

#[tokio::test]
async fn http_auth_open_mode_allows_any() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", "Bearer anything".parse().unwrap());
    let project_id = sigiro::ingest::extract_project_id_http(&AuthMode::Open, &headers)
        .await
        .unwrap();
    assert_eq!(project_id, "default");
}
