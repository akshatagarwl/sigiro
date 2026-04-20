use sigiro::ebpf::{check_obi_available, cleanup_stale_temp_files, ebpf_status, Suggestion};
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

/// Verify cleanup_stale_temp_files:
/// 1. Removes files matching /tmp/sigiro-obi-*.yaml
/// 2. Is idempotent when no files exist
/// Run sequentially (not in parallel with other cleanup tests) to avoid races.
#[test]
fn cleanup_stale_temp_files_behavior() {
    use uuid::Uuid;

    let uid = Uuid::new_v4().to_string()[..8].to_string();
    let tmp1 = PathBuf::from(format!("/tmp/sigiro-obi-test-cleanup-{}-1.yaml", uid));
    let tmp2 = PathBuf::from(format!("/tmp/sigiro-obi-test-cleanup-{}-2.yaml", uid));

    // Pre-cleanup: remove any leftovers from previous failed runs
    let _ = fs::remove_file(&tmp1);
    let _ = fs::remove_file(&tmp2);

    // Create temp files
    fs::write(&tmp1, b"otel_exporter_otlp_endpoint: localhost:4317").unwrap();
    fs::write(&tmp2, b"otel_exporter_otlp_endpoint: localhost:4317").unwrap();
    assert!(tmp1.exists(), "tmp1 should exist before cleanup");
    assert!(tmp2.exists(), "tmp2 should exist before cleanup");

    // Cleanup should remove them
    cleanup_stale_temp_files();
    assert!(!tmp1.exists(), "tmp1 should be removed by cleanup");
    assert!(!tmp2.exists(), "tmp2 should be removed by cleanup");

    // Cleanup should be idempotent (no panic when nothing matches)
    cleanup_stale_temp_files();
    cleanup_stale_temp_files();
}
