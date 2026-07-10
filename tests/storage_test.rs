use sigiro::SignalTable;
use sigiro::storage;

#[test]
fn table_prefix_always_lake() {
    assert_eq!(storage::table_prefix(), "lake.");
}

#[test]
fn signal_table_all_has_all_tables() {
    assert_eq!(SignalTable::ALL.len(), 7);
    assert!(
        SignalTable::ALL
            .iter()
            .any(|t| t.table_suffix() == "sigiro_spans")
    );
    assert!(
        SignalTable::ALL
            .iter()
            .any(|t| t.table_suffix() == "sigiro_logs")
    );
    assert!(
        SignalTable::ALL
            .iter()
            .any(|t| t.table_suffix() == "sigiro_metrics_gauge")
    );
    assert!(
        SignalTable::ALL
            .iter()
            .any(|t| t.table_suffix() == "sigiro_metrics_sum")
    );
    assert!(
        SignalTable::ALL
            .iter()
            .any(|t| t.table_suffix() == "sigiro_metrics_histogram")
    );
    assert!(
        SignalTable::ALL
            .iter()
            .any(|t| t.table_suffix() == "sigiro_metrics_exp_histogram")
    );
    assert!(
        SignalTable::ALL
            .iter()
            .any(|t| t.table_suffix() == "sigiro_profiles")
    );
}

#[test]
fn load_config_returns_none_without_env_vars() {
    // In test environment, SIGIRO_CATALOG_URL is not set
    let config = storage::load_config().unwrap();
    assert!(
        config.is_none(),
        "load_config should return None when SIGIRO_CATALOG_URL is not set"
    );
}

fn r2_config() -> storage::StorageConfig {
    storage::StorageConfig {
        catalog_url: "postgresql://sigiro:secret@catalog.example.com:5432/sigiro".to_string(),
        s3_bucket: "sigiro-tenant-data".to_string(),
        s3_region: "us-east-1".to_string(),
        s3_endpoint: "abc123.r2.cloudflarestorage.com".to_string(),
        s3_key_id: "R2_KEY_ID_EXAMPLE".to_string(),
        s3_secret: "R2_SECRET_EXAMPLE".to_string(),
    }
}

#[test]
fn distributed_attach_sql_scopes_each_tenant_to_distinct_catalog_and_s3_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let config = r2_config();

    let alpha = storage::distributed_attach_sql(
        &config,
        camino::Utf8Path::from_path(dir.path()).unwrap(),
        "alpha",
    )
    .unwrap();
    let beta = storage::distributed_attach_sql(
        &config,
        camino::Utf8Path::from_path(dir.path()).unwrap(),
        "beta",
    )
    .unwrap();

    assert_eq!(alpha.metadata_schema, "tenant_alpha");
    assert_eq!(beta.metadata_schema, "tenant_beta");
    assert_ne!(alpha.attach_sql, beta.attach_sql);
    assert!(alpha.attach_sql.contains("METADATA_SCHEMA 'tenant_alpha'"));
    assert!(
        alpha
            .attach_sql
            .contains("DATA_PATH 's3://sigiro-tenant-data/alpha/'")
    );
    assert!(alpha.attach_sql.contains("CREATE_IF_NOT_EXISTS true"));

    let secret_sql = alpha.secret_sql.as_deref().expect("S3 secret SQL");
    assert!(secret_sql.contains("CREATE OR REPLACE SECRET s3_sigiro"));
    assert!(secret_sql.contains("TYPE S3"));
    assert!(secret_sql.contains("KEY_ID 'R2_KEY_ID_EXAMPLE'"));
    assert!(secret_sql.contains("SECRET 'R2_SECRET_EXAMPLE'"));
    assert!(secret_sql.contains("REGION 'us-east-1'"));
    assert!(secret_sql.contains("ENDPOINT 'abc123.r2.cloudflarestorage.com'"));
    assert!(secret_sql.contains("URL_STYLE 'path'"));
    assert!(secret_sql.contains("SCOPE 's3://sigiro-tenant-data/alpha/'"));
    assert_eq!(
        alpha.s3_scope.as_deref(),
        Some("s3://sigiro-tenant-data/alpha/")
    );
}

#[test]
fn distributed_attach_sql_fails_loud_when_s3_endpoint_missing() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = r2_config();
    config.s3_endpoint = String::new();

    let err = storage::distributed_attach_sql(
        &config,
        camino::Utf8Path::from_path(dir.path()).unwrap(),
        "alpha",
    )
    .unwrap_err();
    assert!(err.to_string().contains("SIGIRO_S3_ENDPOINT"));
}

#[test]
fn distributed_attach_sql_fails_loud_when_s3_credentials_missing() {
    let dir = tempfile::tempdir().unwrap();

    let mut missing_key = r2_config();
    missing_key.s3_key_id = String::new();
    let err = storage::distributed_attach_sql(
        &missing_key,
        camino::Utf8Path::from_path(dir.path()).unwrap(),
        "alpha",
    )
    .unwrap_err();
    assert!(err.to_string().contains("SIGIRO_S3_KEY_ID"));

    let mut missing_secret = r2_config();
    missing_secret.s3_secret = String::new();
    let err = storage::distributed_attach_sql(
        &missing_secret,
        camino::Utf8Path::from_path(dir.path()).unwrap(),
        "alpha",
    )
    .unwrap_err();
    assert!(err.to_string().contains("SIGIRO_S3_SECRET"));
}

#[test]
fn tenant_id_sanitization_rejects_injection_and_path_tokens() {
    for tenant_id in [
        "",
        "tenant-a",
        "tenant/a",
        "../tenant",
        "tenant;DROP_SCHEMA",
        "tenant schema",
        "tenant.schema",
        "tenant'a",
    ] {
        assert!(
            storage::sanitize_tenant_id(tenant_id).is_err(),
            "{tenant_id:?} should be rejected"
        );
    }

    assert_eq!(
        storage::sanitize_tenant_id("tenant_123").unwrap(),
        "tenant_123"
    );
    assert_eq!(
        storage::tenant_metadata_schema("tenant_123").unwrap(),
        "tenant_tenant_123"
    );
}

#[test]
fn sanitize_tenant_id_rejects_reserved_ids() {
    for reserved in ["default", "extensions", "tenants"] {
        assert!(
            storage::sanitize_tenant_id(reserved).is_err(),
            "{reserved:?} collides with a fixed directory name and must be rejected"
        );
    }
}

#[test]
fn tenant_data_dir_nests_under_tenants() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = camino::Utf8Path::from_path(dir.path()).unwrap();

    let path = storage::tenant_data_dir(data_dir, "tenant_123").unwrap();
    assert_eq!(path, data_dir.join("tenants").join("tenant_123"));

    for reserved in ["default", "extensions", "tenants"] {
        assert!(
            storage::tenant_data_dir(data_dir, reserved).is_err(),
            "{reserved:?} must be rejected via sanitize_tenant_id"
        );
    }
}
