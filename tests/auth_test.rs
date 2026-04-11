use rusqlite::Connection;
use sigiro::admin;
use sigiro::auth::*;
use std::sync::Mutex;

fn test_admin_conn() -> Mutex<Connection> {
    let conn = Connection::open_in_memory().unwrap();
    admin::create_keys_table(&conn).unwrap();
    Mutex::new(conn)
}

#[test]
fn extract_bearer_token_valid() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", "Bearer sigiro_abc_defghi".parse().unwrap());
    assert_eq!(extract_bearer_token(&headers).unwrap(), "sigiro_abc_defghi");
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
fn create_and_verify_key() {
    let conn = test_admin_conn();
    let raw_key = admin::create_key(&conn, "proj_test", "free", 100).unwrap();

    assert!(raw_key.starts_with("sigiro_"));

    let (project_id, plan) = admin::verify_key_db(&conn, &raw_key).unwrap();
    assert_eq!(project_id, "proj_test");
    assert_eq!(plan, "free");
}

#[test]
fn verify_invalid_key_fails() {
    let conn = test_admin_conn();
    admin::create_key(&conn, "proj_test", "free", 100).unwrap();

    // Completely wrong key
    assert!(admin::verify_key_db(&conn, "sigiro_AAAAAAAA_BBBBBBBBBBBBBBBBBBBBBBBB").is_err());
}

#[test]
fn verify_nonexistent_short_token() {
    let conn = test_admin_conn();
    assert!(admin::verify_key_db(&conn, "sigiro_ZZZZZZZZ_BBBBBBBBBBBBBBBBBBBBBBBB").is_err());
}

#[test]
fn verify_malformed_key() {
    let conn = test_admin_conn();
    assert!(admin::verify_key_db(&conn, "not-a-valid-key").is_err());
}

#[test]
fn revoke_key_works() {
    let conn = test_admin_conn();
    let raw_key = admin::create_key(&conn, "proj_test", "free", 100).unwrap();

    // Extract short token from key
    let pak = prefixed_api_key::PrefixedApiKey::from_string(&raw_key).unwrap();
    let short = pak.short_token().to_string();

    assert!(admin::revoke_key(&conn, &short).unwrap());
    assert!(admin::verify_key_db(&conn, &raw_key).is_err());
}

#[test]
fn revoke_nonexistent_returns_false() {
    let conn = test_admin_conn();
    assert!(!admin::revoke_key(&conn, "nonexistent").unwrap());
}

#[test]
fn multiple_keys_per_project() {
    let conn = test_admin_conn();
    let key1 = admin::create_key(&conn, "proj_a", "free", 100).unwrap();
    let key2 = admin::create_key(&conn, "proj_a", "builder", 500).unwrap();

    let (pid1, plan1) = admin::verify_key_db(&conn, &key1).unwrap();
    let (pid2, plan2) = admin::verify_key_db(&conn, &key2).unwrap();
    assert_eq!(pid1, "proj_a");
    assert_eq!(pid2, "proj_a");
    assert_eq!(plan1, "free");
    assert_eq!(plan2, "builder");
}

#[test]
fn dev_key_prefix_recognized() {
    assert!("sigiro_dev_test".starts_with("sigiro_dev_"));
    assert!(!"sigiro_BRTRKFsL_xxx".starts_with("sigiro_dev_"));
}

#[test]
fn verify_key_cached_returns_same_result() {
    let conn = test_admin_conn();
    let raw_key = admin::create_key(&conn, "proj_cached", "free", 100).unwrap();

    // First call — cache miss, hits SQLite
    let ctx1 = verify_key_cached(&conn, &raw_key).unwrap();
    assert_eq!(ctx1.project_id, "proj_cached");

    // Second call — cache hit, should return same result
    let ctx2 = verify_key_cached(&conn, &raw_key).unwrap();
    assert_eq!(ctx2.project_id, "proj_cached");
    assert_eq!(ctx2.plan, "free");
}

#[test]
fn verify_key_cached_invalid_key_not_cached() {
    let conn = test_admin_conn();

    // Invalid key should fail
    assert!(verify_key_cached(&conn, "sigiro_AAAAAAAA_BBBBBBBBBBBBBBBBBBBBBBBB").is_err());

    // Should fail again (not cached as success)
    assert!(verify_key_cached(&conn, "sigiro_AAAAAAAA_BBBBBBBBBBBBBBBBBBBBBBBB").is_err());
}
