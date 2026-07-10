use sigiro::auth::*;
use std::sync::Arc;

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

#[test]
fn tenant_key_auth_resolves_valid_key_and_rejects_revoked_or_unknown_keys() {
    let control = Arc::new(ControlPlane::open_in_memory_for_tests().unwrap());
    control.create_tenant("tenant_a").unwrap();
    let minted = control.mint_key("tenant_a").unwrap();

    let mode = AuthMode::TenantKeys {
        control: Arc::clone(&control),
    };
    let ctx = mode.verify(&minted.key).unwrap();
    assert_eq!(ctx.tenant_id, "tenant_a");
    assert_eq!(ctx.project_id, "tenant_a");
    assert_ne!(minted.key_hash, minted.key);
    assert_eq!(minted.key_hash, hash_api_key(&minted.key));

    assert!(mode.verify("sk_tenant_a_unknown").is_err());

    let revoked = control
        .revoke_key_hash_prefix(&minted.key_hash[..12])
        .unwrap();
    assert!(revoked);
    assert!(mode.verify(&minted.key).is_err());
}
