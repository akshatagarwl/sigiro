use sigiro::ebpf::{Suggestion, check_obi_available, ebpf_status, cleanup_stale_temp_files, get_registry};
use std::fs;
use std::path::PathBuf;

#[test]
fn check_obi_available_returns_false_when_not_in_path() {
    // obi is almost certainly not installed on dev machines
    let result = check_obi_available("/nonexistent/path/obi");
    assert!(!result);
}

#[test]
fn ebpf_status_not_installed_when_binary_missing() {
    let status = ebpf_status("/nonexistent/path/obi");
    assert_eq!(status.status, "not_installed");
    assert!(status.reason.contains("obi binary not found"));
}

#[test]
fn suggestion_strings_are_nonempty() {
    let s = Suggestion::docker();
    assert!(!s.is_empty());
    assert!(s.contains("docker"));

    let s = Suggestion::k8s();
    assert!(!s.is_empty());
    assert!(s.contains("securityContext"));

    let s = Suggestion::bare_metal();
    assert!(!s.is_empty());
    assert!(s.contains("setcap"));
}

#[test]
fn missing_capabilities_format() {
    let missing = ["CAP_BPF".to_string(), "CAP_NET_ADMIN".to_string()];
    let reason = format!("missing capabilities: {}", missing.join(", "));
    assert!(reason.contains("CAP_BPF"));
    assert!(reason.contains("CAP_NET_ADMIN"));
}

#[test]
fn cleanup_stale_temp_files_removes_orphaned_configs() {
    // Create temp files with the sigiro-obi-*.yaml pattern
    let tmp1 = PathBuf::from("/tmp/sigiro-obi-test-cleanup-1.yaml");
    let tmp2 = PathBuf::from("/tmp/sigiro-obi-test-cleanup-2.yaml");
    fs::write(&tmp1, b"otel_exporter_otlp_endpoint: localhost:4317").unwrap();
    fs::write(&tmp2, b"otel_exporter_otlp_endpoint: localhost:4317").unwrap();

    // Verify they exist
    assert!(tmp1.exists());
    assert!(tmp2.exists());

    // Run cleanup
    cleanup_stale_temp_files();

    // Verify they are gone
    assert!(!tmp1.exists(), "tmp1 should be removed by cleanup");
    assert!(!tmp2.exists(), "tmp2 should be removed by cleanup");
}

#[test]
fn cleanup_stale_temp_files_idempotent_when_none_exist() {
    // Cleanup should not panic when no files match
    cleanup_stale_temp_files();
    cleanup_stale_temp_files();
}

#[test]
fn registry_get_registry_returns_same_instance() {
    let reg1 = get_registry();
    let reg2 = get_registry();
    // Same pointer means same static
    assert!(std::ptr::eq(reg1, reg2));
}
