use sigiro::auth::{AuthMode, ControlPlane};
use std::sync::Arc;

/// Build a `TenantKeys` auth mode over an in-memory control plane with one
/// active tenant, returning the mode plus a freshly minted key that resolves to
/// that tenant. This is the only mode that requires a Bearer token now that the
/// legacy static-key mode is gone (self-host/edge is `Open`).
fn tenant_keys_mode(tenant: &str) -> (AuthMode, String) {
    let control = Arc::new(ControlPlane::open_in_memory_for_tests().unwrap());
    control.create_tenant(tenant).unwrap();
    let minted = control.mint_key(tenant).unwrap();
    (AuthMode::TenantKeys { control }, minted.key)
}

#[test]
fn grpc_auth_open_mode_allows_any() {
    let mut metadata = tonic::metadata::MetadataMap::new();
    metadata.insert("authorization", "Bearer anything".parse().unwrap());
    let tenant_id = sigiro::ingest::extract_tenant_id_grpc(&AuthMode::Open, &metadata).unwrap();
    assert_eq!(tenant_id, "default");
}

#[test]
fn grpc_auth_missing_header_fails() {
    let (mode, _key) = tenant_keys_mode("acme");
    let metadata = tonic::metadata::MetadataMap::new();
    let err = sigiro::ingest::extract_tenant_id_grpc(&mode, &metadata).unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
}

#[test]
fn grpc_auth_invalid_key_fails() {
    let (mode, _key) = tenant_keys_mode("acme");
    let mut metadata = tonic::metadata::MetadataMap::new();
    metadata.insert("authorization", "Bearer sk_acme_deadbeef".parse().unwrap());
    let err = sigiro::ingest::extract_tenant_id_grpc(&mode, &metadata).unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
}

#[test]
fn grpc_auth_matching_key_resolves_tenant() {
    let (mode, key) = tenant_keys_mode("acme");
    let mut metadata = tonic::metadata::MetadataMap::new();
    metadata.insert("authorization", format!("Bearer {key}").parse().unwrap());
    let tenant_id = sigiro::ingest::extract_tenant_id_grpc(&mode, &metadata).unwrap();
    assert_eq!(tenant_id, "acme");
}

#[test]
fn http_auth_open_mode_allows_any() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", "Bearer anything".parse().unwrap());
    let tenant_id = sigiro::ingest::extract_tenant_id_http(&AuthMode::Open, &headers).unwrap();
    assert_eq!(tenant_id, "default");
}

#[test]
fn http_auth_missing_header_returns_401() {
    let (mode, _key) = tenant_keys_mode("acme");
    let headers = axum::http::HeaderMap::new();
    let err = sigiro::ingest::extract_tenant_id_http(&mode, &headers).unwrap_err();
    assert_eq!(err, axum::http::StatusCode::UNAUTHORIZED);
}

#[test]
fn http_auth_invalid_key_returns_401() {
    let (mode, _key) = tenant_keys_mode("acme");
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", "Bearer sk_acme_deadbeef".parse().unwrap());
    let err = sigiro::ingest::extract_tenant_id_http(&mode, &headers).unwrap_err();
    assert_eq!(err, axum::http::StatusCode::UNAUTHORIZED);
}

#[test]
fn http_auth_matching_key_resolves_tenant() {
    let (mode, key) = tenant_keys_mode("acme");
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", format!("Bearer {key}").parse().unwrap());
    let tenant_id = sigiro::ingest::extract_tenant_id_http(&mode, &headers).unwrap();
    assert_eq!(tenant_id, "acme");
}
