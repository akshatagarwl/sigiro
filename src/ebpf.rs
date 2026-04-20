use anyhow::{Context, Result};
use std::process::Command;
use tracing::{debug, info, warn};

/// Result of checking eBPF readiness.
pub struct EbpfStatus {
    pub status: String,
    pub reason: String,
    pub suggestions: Vec<String>,
}

/// Check if obi binary exists at the given path or in PATH.
pub fn check_obi_available(obi_path: &str) -> bool {
    if obi_path != "obi" && !obi_path.is_empty() {
        // Explicit path — check file exists
        std::path::Path::new(obi_path).exists()
    } else {
        // Check PATH
        which_obi().is_some()
    }
}

/// Find obi in PATH using the `which` crate.
fn which_obi() -> Option<String> {
    which::which("obi")
        .inspect_err(|e| debug!("obi not found in PATH: {e}"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// Determine eBPF status: check obi availability and platform capabilities.
pub fn ebpf_status(obi_path: &str) -> EbpfStatus {
    if !check_obi_available(obi_path) {
        return EbpfStatus {
            status: "not_installed".to_string(),
            reason: "obi binary not found".to_string(),
            suggestions: vec![
                Suggestion::docker(),
                Suggestion::k8s(),
                Suggestion::bare_metal(),
            ],
        };
    }

    // Platform capability check (Unix only)
    let missing = check_capabilities();
    if !missing.is_empty() {
        return EbpfStatus {
            status: "disabled".to_string(),
            reason: format!("missing capabilities: {}", missing.join(", ")),
            suggestions: vec![
                Suggestion::docker(),
                Suggestion::k8s(),
                Suggestion::bare_metal(),
            ],
        };
    }

    EbpfStatus {
        status: "ready".to_string(),
        reason: String::new(),
        suggestions: vec![],
    }
}

/// Check Unix capabilities needed for eBPF.
/// Returns list of missing capability names.
fn check_capabilities() -> Vec<String> {
    let mut missing = Vec::new();
    let is_root = is_root();
    let has_bpf = has_capability("CAP_BPF");
    let has_net_admin = has_capability("CAP_NET_ADMIN");

    if !is_root && !has_bpf {
        missing.push("CAP_BPF".to_string());
    }
    if !is_root && !has_net_admin {
        missing.push("CAP_NET_ADMIN".to_string());
    }
    missing
}

/// Check if running as root (euid == 0).
fn is_root() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: geteuid() is a pure read of the effective user ID.
        // It has no side effects and returns the same value for all threads,
        // so there are no data races or undefined behavior invariants to uphold.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Check if the process has a specific Linux capability.
fn has_capability(cap: &str) -> bool {
    // Root always has all capabilities
    if is_root() {
        return true;
    }

    // On Linux, check /proc/self/status for CapEff
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if let Some(hex) = line.strip_prefix("CapEff:") {
                    if let Ok(bits) = u64::from_str_radix(hex.trim(), 16) {
                        return match cap {
                            "CAP_BPF" => bits & (1 << 39) != 0,
                            "CAP_NET_ADMIN" => bits & (1 << 12) != 0,
                            "CAP_SYS_ADMIN" => bits & (1 << 21) != 0,
                            _ => false,
                        };
                    }
                }
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = cap;
    }
    false
}

/// Try to start obi as a subprocess.
/// Returns Ok(()) if obi started successfully.
/// Returns Err if obi is ready but failed to spawn — this is a real error, not graceful degradation.
/// Callers handle not_installed/disabled themselves (call ebpf_status first).
pub fn start_obi(obi_path: &str, otlp_endpoint: &str) -> Result<()> {
    let path = if obi_path.is_empty() { "obi" } else { obi_path };
    info!("starting obi with OTLP endpoint: {otlp_endpoint}");
    let mut child = Command::new(path)
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", otlp_endpoint)
        .spawn()
        .context("obi failed to start — check that obi is installed and executable")?;

    info!("obi started (pid: {:?})", child.id());
    std::thread::spawn(move || {
        if let Ok(exit) = child.wait() {
            warn!("obi exited: {exit}");
        }
    });
    Ok(())
}

/// Write eBPF status to instance_status.
pub fn write_ebpf_status(conn: &rusqlite::Connection, status: &EbpfStatus) -> Result<()> {
    crate::admin::write_instance_status(conn, "ebpf_status", &status.status)?;
    crate::admin::write_instance_status(conn, "ebpf_reason", &status.reason)?;
    crate::admin::write_instance_status(
        conn,
        "missing_permissions",
        &format!("{:?}", status.suggestions),
    )?;
    if let Some(s) = status.suggestions.first() {
        crate::admin::write_instance_status(conn, "suggestion_docker", s)?;
    }
    Ok(())
}

/// Suggestion strings for eBPF setup on different platforms.
pub struct Suggestion;

impl Suggestion {
    pub fn docker() -> String {
        "docker run --cap-add BPF --cap-add NET_ADMIN ...".to_string()
    }
    pub fn k8s() -> String {
        "securityContext:\n  capabilities:\n    add: [BPF, NET_ADMIN, SYS_ADMIN]".to_string()
    }
    pub fn bare_metal() -> String {
        "sudo setcap cap_bpf,cap_net_admin+ep /usr/local/bin/obi".to_string()
    }
}
