use sigiro::auth::*;
use std::sync::Arc;

#[test]
fn api_key_hash_prefix_checks_construction_and_serde_without_changing_wire_shape() {
    use sigiro::types::ApiKeyHashPrefix;

    for invalid in ["abc1234".to_owned(), "abc1234!".to_owned(), "a".repeat(65)] {
        assert!(ApiKeyHashPrefix::try_new(invalid.clone()).is_err());
        assert!(serde_json::from_str::<ApiKeyHashPrefix>(&format!("\"{invalid}\"")).is_err());
    }

    let prefix = ApiKeyHashPrefix::try_new("ABCDEF12".to_owned()).unwrap();
    assert_eq!(serde_json::to_string(&prefix).unwrap(), "\"ABCDEF12\"");
    assert_eq!(
        serde_json::from_str::<ApiKeyHashPrefix>("\"ABCDEF12\"").unwrap(),
        prefix
    );
}

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
    let tenant_id: sigiro::types::TenantId = mode.verify("anything").unwrap();
    assert_eq!(tenant_id.as_ref(), "default");
}

/// No `SIGIRO_CATALOG_URL` (local self-host) ⇒ `Open` (no per-request auth).
#[test]
fn auth_mode_local_no_catalog_is_open() {
    let dir = camino::Utf8PathBuf::from("/tmp/sigiro-auth-test");
    let mode = AuthMode::from_runtime_config(
        &dir,
        sigiro::config::QuerySurface::None,
        &sigiro::storage::StorageMode::LocalFile,
    )
    .unwrap();
    assert!(matches!(mode, AuthMode::Open));
    assert!(!mode.requires_auth());
}

/// A quack edge stays `Open` (perimeter-protected) regardless of storage — it
/// short-circuits before the distributed control-plane branch.
#[test]
fn auth_mode_quack_edge_is_open() {
    let dir = camino::Utf8PathBuf::from("/tmp/sigiro-auth-test");
    let mode = AuthMode::from_runtime_config(
        &dir,
        sigiro::config::QuerySurface::Quack,
        &sigiro::storage::StorageMode::LocalFile,
    )
    .unwrap();
    assert!(matches!(mode, AuthMode::Open));
    assert!(!mode.requires_auth());
}

// The hosted `SIGIRO_CATALOG_URL` ⇒ `TenantKeys` selection opens a real
// PostgreSQL control catalog, so it is exercised end-to-end (against Dockerized
// Postgres) in `control_plane_e2e_test.rs::control_plane_lifecycle`, not here.

#[test]
fn tenant_key_auth_resolves_valid_key_and_rejects_revoked_or_unknown_keys() {
    let control = Arc::new(ControlPlane::open_in_memory_for_tests().unwrap());
    control.create_tenant("tenant_a").unwrap();
    let minted = control.mint_key("tenant_a").unwrap();

    let mode = AuthMode::TenantKeys {
        control: Arc::clone(&control),
    };
    let tenant_id: sigiro::types::TenantId = mode.verify(&minted.key).unwrap();
    assert_eq!(tenant_id.as_ref(), "tenant_a");
    assert_ne!(minted.key_hash, minted.key);
    assert_eq!(minted.key_hash, hash_api_key(&minted.key));

    assert!(mode.verify("sk_tenant_a_unknown").is_err());

    let revoked = control
        .revoke_key_hash_prefix(
            &sigiro::types::ApiKeyHashPrefix::try_new(minted.key_hash[..12].to_owned()).unwrap(),
        )
        .unwrap();
    assert!(revoked);
    assert!(mode.verify(&minted.key).is_err());
}

/// Per-mode "default" resolution: the name `"default"` stays valid for the
/// local/edge project (`sanitize_tenant_id` accepts it — the edge pre-creates
/// it), but the HOSTED control plane must fail loud when an operator tries to
/// provision a tenant literally named `default`.
#[test]
fn default_is_local_valid_but_rejected_by_hosted_control_plane() {
    // Local/edge path: still accepts "default" byte-for-byte.
    assert_eq!(
        sigiro::storage::sanitize_tenant_id("default")
            .unwrap()
            .as_ref(),
        "default"
    );

    let control = ControlPlane::open_in_memory_for_tests().unwrap();

    // Hosted provisioning rejects "default" on BOTH create and mint.
    let create_err = control.create_tenant("default").unwrap_err();
    assert!(
        format!("{create_err:#}").contains("reserved"),
        "expected a reserved-name rejection, got: {create_err:#}"
    );
    assert!(control.mint_key("default").is_err());

    // A normal hosted tenant is unaffected.
    control.create_tenant("acme").unwrap();
    control.mint_key("acme").unwrap();

    // The globally-reserved names stay rejected everywhere.
    assert!(control.create_tenant("tenants").is_err());
    assert!(sigiro::storage::sanitize_tenant_id("extensions").is_err());
}
