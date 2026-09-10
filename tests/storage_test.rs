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

// --- Storage-mode config-axis validation (StorageMode::from_env) -------------
//
// SIGIRO_CATALOG_URL is an `on_startup` (process-cached) var and is never set
// in this parent test process, so `from_env` reliably sees "no catalog URL".
// SIGIRO_S3_BUCKET is `on_demand`, so `temp_env` can toggle it per test. Both
// tests go through `temp_env` (which serializes on a global lock) so the S3
// bucket state can't leak between concurrently-running tests.

#[test]
fn from_env_is_local_file_without_catalog_or_bucket() {
    temp_env::with_var_unset("SIGIRO_S3_BUCKET", || {
        let mode = storage::StorageMode::from_env().expect("local-file mode must resolve");
        assert!(matches!(mode, storage::StorageMode::LocalFile));
        assert!(!mode.is_distributed());
        assert!(!mode.uses_s3());
        assert!(mode.distributed_config().is_none());
    });
}

#[test]
fn from_env_fails_loud_when_s3_bucket_set_without_catalog_url() {
    temp_env::with_var("SIGIRO_S3_BUCKET", Some("sigiro-data"), || {
        let err = storage::StorageMode::from_env()
            .expect_err("S3 bucket without a catalog URL must fail the coherence gate");
        let msg = err.to_string();
        assert!(
            msg.contains("SIGIRO_S3_BUCKET") && msg.contains("SIGIRO_CATALOG_URL"),
            "error must name both incoherent vars, got: {msg}"
        );
    });
}

fn r2_config() -> storage::StorageConfig {
    s3_config(
        "https://abc123.r2.cloudflarestorage.com",
        "R2_KEY_ID_EXAMPLE",
        "R2_SECRET_EXAMPLE",
    )
    .unwrap()
}

fn s3_config(endpoint: &str, key_id: &str, secret: &str) -> anyhow::Result<storage::StorageConfig> {
    let s3 = storage::S3Config::new(
        storage::S3BucketName::try_new("sigiro-tenant-data".to_string())?,
        storage::S3Region::try_new("us-east-1".to_string())?,
        endpoint.parse()?,
        key_id.to_string(),
        secret.to_string(),
    )?;
    storage::StorageConfig::new(
        "postgresql://sigiro:***@catalog.example.com:5432/sigiro".to_string(),
        storage::DataLocation::S3(s3),
    )
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
    assert!(secret_sql.contains("USE_SSL true"));
    assert!(secret_sql.contains("SCOPE 's3://sigiro-tenant-data/alpha/'"));
    assert_eq!(
        alpha.s3_scope.as_deref(),
        Some("s3://sigiro-tenant-data/alpha/")
    );
}

#[test]
fn distributed_attach_sql_use_ssl_true_for_real_s3() {
    let dir = tempfile::tempdir().unwrap();
    let config = r2_config();

    let attach = storage::distributed_attach_sql(
        &config,
        camino::Utf8Path::from_path(dir.path()).unwrap(),
        "alpha",
    )
    .unwrap();
    let secret_sql = attach.secret_sql.as_deref().expect("S3 secret SQL");
    assert!(
        secret_sql.contains("USE_SSL true"),
        "real S3/R2 must default to TLS: {secret_sql}"
    );
}

#[test]
fn distributed_attach_sql_use_ssl_false_for_dev_rustfs() {
    let dir = tempfile::tempdir().unwrap();
    let config = s3_config(
        "http://127.0.0.1:9000",
        "R2_KEY_ID_EXAMPLE",
        "R2_SECRET_EXAMPLE",
    )
    .unwrap();

    let attach = storage::distributed_attach_sql(
        &config,
        camino::Utf8Path::from_path(dir.path()).unwrap(),
        "alpha",
    )
    .unwrap();
    let secret_sql = attach.secret_sql.as_deref().expect("S3 secret SQL");
    assert!(
        secret_sql.contains("USE_SSL false"),
        "an HTTP endpoint must produce USE_SSL false for plain-HTTP dev stores: {secret_sql}"
    );
    assert!(secret_sql.contains("ENDPOINT '127.0.0.1:9000'"));
}

#[test]
fn s3_config_rejects_non_origin_endpoints() {
    for endpoint in [
        "ftp://s3.example.com",
        "https://user@s3.example.com",
        "https://s3.example.com/path",
        "https://s3.example.com?query=yes",
        "https://s3.example.com#fragment",
    ] {
        let err = s3_config(endpoint, "key", "secret").unwrap_err();
        assert!(
            err.to_string().contains("SIGIRO_S3_ENDPOINT"),
            "endpoint {endpoint:?} should fail as a non-origin URL: {err}"
        );
    }
}

#[test]
fn s3_config_rejects_missing_credentials() {
    let err = s3_config("https://s3.example.com", "", "secret").unwrap_err();
    assert!(err.to_string().contains("SIGIRO_S3_KEY_ID"));

    let err = s3_config("https://s3.example.com", "key", "").unwrap_err();
    assert!(err.to_string().contains("SIGIRO_S3_SECRET"));
}

#[test]
fn s3_bucket_and_region_reject_empty_values() {
    assert!(storage::S3BucketName::try_new("  ".to_string()).is_err());
    assert!(storage::S3Region::try_new("  ".to_string()).is_err());

    for bucket in [
        "bucket/prefix",
        "bucket:port",
        "bucket@host",
        "bucket\nname",
    ] {
        assert!(
            storage::S3BucketName::try_new(bucket.to_string()).is_err(),
            "bucket {bucket:?} must not alter the S3 URI authority"
        );
    }
}

#[test]
fn storage_config_rejects_empty_catalog() {
    assert!(
        storage::StorageConfig::new("  ".to_string(), storage::DataLocation::LocalFilesystem)
            .is_err()
    );
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
        storage::sanitize_tenant_id("tenant_123").unwrap().as_ref(),
        "tenant_123"
    );
    assert_eq!(
        storage::tenant_metadata_schema("tenant_123").unwrap(),
        "tenant_tenant_123"
    );
}

#[test]
fn sanitize_tenant_id_rejects_reserved_ids() {
    for reserved in ["extensions", "tenants"] {
        assert!(
            storage::sanitize_tenant_id(reserved).is_err(),
            "{reserved:?} collides with a fixed directory name and must be rejected"
        );
    }

    // "default" is NOT reserved — it names a normal distributed tenant,
    // independently namespaced from the local-mode "default project" dir
    // (see sanitize_tenant_id's doc comment).
    assert_eq!(
        storage::sanitize_tenant_id("default").unwrap().as_ref(),
        "default"
    );
}

#[test]
fn tenant_data_dir_nests_under_tenants() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = camino::Utf8Path::from_path(dir.path()).unwrap();

    let path = storage::tenant_data_dir(data_dir, "tenant_123").unwrap();
    assert_eq!(path, data_dir.join("tenants").join("tenant_123"));

    for reserved in ["extensions", "tenants"] {
        assert!(
            storage::tenant_data_dir(data_dir, reserved).is_err(),
            "{reserved:?} must be rejected via sanitize_tenant_id"
        );
    }

    // "default" is allowed and nests the same as any other tenant id.
    assert_eq!(
        storage::tenant_data_dir(data_dir, "default").unwrap(),
        data_dir.join("tenants").join("default")
    );
}
