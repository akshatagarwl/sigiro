use sigiro::auth::*;

#[test]
fn extract_bearer_token_valid() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", "Bearer my_secret_key".parse().unwrap());
    assert_eq!(extract_bearer_token(&headers).unwrap(), "my_secret_key");
}

#[test]
fn extract_bearer_token_missing() {
    let headers = axum::http::HeaderMap::new();
    assert!(extract_bearer_token(&headers).is_err());
}

#[test]
fn extract_bearer_token_malformed() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", "Basic xyz".parse().unwrap());
    assert!(extract_bearer_token(&headers).is_err());
}

/// No `SIGIRO_CATALOG_URL` (local self-host) ⇒ `Open` (no per-request auth).
#[tokio::test]
async fn auth_mode_local_no_catalog_is_open() {
    let mode = AuthMode::from_runtime_config(
        sigiro::config::QuerySurface::None,
        &sigiro::storage::StorageMode::LocalFile,
    )
    .await
    .unwrap();
    assert!(matches!(mode, AuthMode::Open));
    assert!(!mode.requires_auth());
}

/// A quack edge stays `Open` (perimeter-protected) regardless of storage — it
/// short-circuits before hosted authentication selection.
#[tokio::test]
async fn auth_mode_quack_edge_is_open() {
    let mode = AuthMode::from_runtime_config(
        sigiro::config::QuerySurface::Quack,
        &sigiro::storage::StorageMode::LocalFile,
    )
    .await
    .unwrap();
    assert!(matches!(mode, AuthMode::Open));
    assert!(!mode.requires_auth());
}
