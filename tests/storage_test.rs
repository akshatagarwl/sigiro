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
