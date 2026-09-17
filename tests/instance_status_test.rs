use sigiro::admin::{
    read_all_status, test_connection, write_instance_status, write_instance_status_batch,
};

fn test_conn() -> sigiro::admin::AdminConnection {
    test_connection().unwrap()
}

#[test]
fn create_table_and_write_keys() {
    let mut conn = test_conn();
    write_instance_status(&mut conn, "version", "0.1.0").unwrap();
    write_instance_status(&mut conn, "mode", "otlp").unwrap();
    write_instance_status(&mut conn, "platform_os", "linux").unwrap();
    write_instance_status(&mut conn, "platform_arch", "x86_64").unwrap();
    write_instance_status(&mut conn, "platform_kernel", "6.8.0").unwrap();

    let rows = read_all_status(&mut conn).unwrap();
    assert_eq!(rows.len(), 5);
    assert_eq!(rows.get("version").unwrap(), "0.1.0");
    assert_eq!(rows.get("mode").unwrap(), "otlp");
    assert_eq!(rows.get("platform_kernel").unwrap(), "6.8.0");
}

#[test]
fn upsert_updates_existing_key() {
    let mut conn = test_conn();
    write_instance_status(&mut conn, "version", "0.1.0").unwrap();
    write_instance_status(&mut conn, "version", "0.2.0").unwrap();

    let rows = read_all_status(&mut conn).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows.get("version").unwrap(), "0.2.0");
}

/// The batch write must produce byte-identical rows to the same keys written
/// one at a time. Boot switched from 13 individual writes to one batch, so a
/// divergence here silently changes what /health and diagnose blocks report.
#[test]
fn batch_matches_individual_writes() {
    let entries: &[(&str, &str)] = &[
        ("version", "0.1.0"),
        ("mode", "otlp"),
        ("platform_os", "linux"),
        ("platform_arch", "x86_64"),
        ("platform_kernel", "6.8.0"),
    ];

    let mut singles = test_conn();
    for (k, v) in entries {
        write_instance_status(&mut singles, k, v).unwrap();
    }

    let mut batched = test_conn();
    write_instance_status_batch(&mut batched, entries).unwrap();

    assert_eq!(
        read_all_status(&mut singles).unwrap(),
        read_all_status(&mut batched).unwrap()
    );
}

/// The batch shares the upsert path, so a repeated key must overwrite rather
/// than fail the whole transaction on the PRIMARY KEY conflict.
#[test]
fn batch_upserts_existing_keys() {
    let mut conn = test_conn();
    write_instance_status(&mut conn, "version", "0.1.0").unwrap();
    write_instance_status_batch(&mut conn, &[("version", "0.2.0"), ("mode", "otlp")]).unwrap();

    let rows = read_all_status(&mut conn).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows.get("version").unwrap(), "0.2.0");
}

/// An empty batch must be a no-op, not an error. `run_serve` builds the slice
/// from probe results, so an all-empty case must not fail boot.
#[test]
fn batch_with_no_entries_is_a_noop() {
    let mut conn = test_conn();
    write_instance_status_batch(&mut conn, &[]).unwrap();
    assert!(read_all_status(&mut conn).unwrap().is_empty());
}

#[test]
fn empty_table_returns_empty_map() {
    let mut conn = test_conn();
    let rows = read_all_status(&mut conn).unwrap();
    assert!(rows.is_empty());
}
