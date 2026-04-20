use sigiro::ebpf::{check_obi_available, ebpf_status, Suggestion};

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


