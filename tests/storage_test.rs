use sigiro::storage;

#[test]
fn table_prefix_always_lake() {
    assert_eq!(storage::table_prefix(), "lake.");
}

#[test]
fn table_names_has_all_six() {
    assert_eq!(storage::TABLE_NAMES.len(), 6);
    assert!(storage::TABLE_NAMES.contains(&"sigiro_spans"));
    assert!(storage::TABLE_NAMES.contains(&"sigiro_logs"));
    assert!(storage::TABLE_NAMES.contains(&"sigiro_metrics_gauge"));
    assert!(storage::TABLE_NAMES.contains(&"sigiro_metrics_sum"));
    assert!(storage::TABLE_NAMES.contains(&"sigiro_metrics_histogram"));
    assert!(storage::TABLE_NAMES.contains(&"sigiro_metrics_exp_histogram"));
}

#[test]
fn load_config_returns_none_without_env_vars() {
    // In test environment, SIGIRO_CATALOG_URL is not set
    let config = storage::load_config();
    assert!(
        config.is_none(),
        "load_config should return None when SIGIRO_CATALOG_URL is not set"
    );
}
