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
fn auth_mode_open_allows_any() {
    let mode = AuthMode::Open;
    let ctx = mode.verify("anything").unwrap();
    assert_eq!(ctx.project_id, "default");
}

#[test]
fn auth_mode_key_matching_succeeds() {
    let mode = AuthMode::Key {
        key: "my_secret".to_string(),
    };
    let ctx = mode.verify("my_secret").unwrap();
    assert_eq!(ctx.project_id, "default");
}

#[test]
fn auth_mode_key_wrong_fails() {
    let mode = AuthMode::Key {
        key: "my_secret".to_string(),
    };
    assert!(mode.verify("wrong_key").is_err());
}

#[test]
fn auth_mode_key_empty_token_fails() {
    let mode = AuthMode::Key {
        key: "my_secret".to_string(),
    };
    assert!(mode.verify("").is_err());
}

#[test]
fn auth_mode_from_config_empty_is_open() {
    let mode = AuthMode::from_config(String::new());
    assert!(matches!(mode, AuthMode::Open));
    assert!(!mode.requires_auth());
}

#[test]
fn auth_mode_from_config_non_empty_is_key() {
    let mode = AuthMode::from_config("secret".to_string());
    assert!(matches!(mode, AuthMode::Key { .. }));
    assert!(mode.requires_auth());
}
