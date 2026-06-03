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

#[test]
fn verify_key_open_mode_allows_any() {
    let ctx = verify_key("", "anything").unwrap();
    assert_eq!(ctx.project_id, "default");
}

#[test]
fn verify_key_matching_key_succeeds() {
    let ctx = verify_key("my_secret", "my_secret").unwrap();
    assert_eq!(ctx.project_id, "default");
}

#[test]
fn verify_key_wrong_key_fails() {
    assert!(verify_key("my_secret", "wrong_key").is_err());
}

#[test]
fn verify_key_empty_token_fails_when_key_set() {
    assert!(verify_key("my_secret", "").is_err());
}
