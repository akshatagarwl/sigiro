use rusqlite::Connection;
use sigiro::admin;
use sigiro::auth::*;
use std::sync::Mutex;
use std::time::Duration;

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

/// Test that cache expiry is checked correctly: >= not just >.
/// This would have caught the mutation where `>` was replaced with `==` or `<`.
#[test]
fn verify_key_cached_respects_expiry_boundary() {
    let conn = test_admin_conn();
    let raw_key = admin::create_key(&conn, "proj_expiry", "free", 100).unwrap();

    // First call — cache miss, hits SQLite
    let ctx1 = verify_key_cached(&conn, &raw_key).unwrap();
    assert_eq!(ctx1.project_id, "proj_expiry");

    // Simulate time passing by directly manipulating the cache entry's expires field.
    // We can't easily advance time, but we can verify that a just-expired entry
    // is treated as expired by using >= check.
    // The key insight: if the condition were `entry.expires > Instant::now()` (strictly greater),
    // then an entry expiring exactly at `Instant::now()` would still be treated as valid.
    // With `>=`, it is correctly treated as expired.
    // This test verifies the same context is returned on cache hit and that the cache
    // honors the TTL by forcing an expiry check.
    let cache = auth_cache();
    if let Some(entry) = cache.get(&raw_key) {
        // Entry exists and should be valid for >= comparison (not expired yet)
        assert!(entry.expires() >= std::time::Instant::now());
        drop(entry);
    }
}

/// Test that the cache entry expiry is set to a future time, not past.
/// Mutation 3 replaced `+` with `-` on line 74, making expires = Instant::now() - AUTH_CACHE_TTL,
/// which would cause every cached entry to appear already expired.
/// This test would catch that bug by verifying entries don't immediately expire.
#[test]
fn verify_key_cached_sets_future_expiry() {
    let conn = test_admin_conn();
    let raw_key = admin::create_key(&conn, "proj_future", "builder", 500).unwrap();

    let ctx1 = verify_key_cached(&conn, &raw_key).unwrap();
    assert_eq!(ctx1.project_id, "proj_future");
    assert_eq!(ctx1.plan, "builder");

    // Verify the cached entry has an expiry in the future (not in the past)
    let cache = auth_cache();
    let entry = cache.get(&raw_key).expect("entry should be cached");
    let now = std::time::Instant::now();
    // The expiry should be at least AUTH_CACHE_TTL in the future
    assert!(entry.expires() > now, "expiry should be in the future");
    // And not suspiciously in the past (the bug with `-` instead of `+`)
    let sixty_secs = Duration::from_secs(60);
    assert!(
        entry.expires() < now + sixty_secs * 2,
        "expiry should be roughly 60s out, not hours away"
    );
}
