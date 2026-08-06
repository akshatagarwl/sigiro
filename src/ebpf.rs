//! eBPF capability probe — "can this host run eBPF auto-instrumentation".
//!
//! Read-only reporting only; the process that actually runs obi is a future
//! feature (the auto-download/launch machinery was removed as broken).
//!
//! `serve` runs this once at startup and records the verdict into
//! `instance_status`, where `/admin/status` serves it. Nothing here touches
//! telemetry or the query path — it is a host capability check, which is why
//! it lives in its own module rather than inside a reporting command that can
//! be deleted out from under it.

use anyhow::Result;

#[cfg(target_os = "linux")]
use caps::{CapSet, Capability};

/// Result of checking eBPF readiness.
pub struct EbpfStatus {
    pub status: String,
    pub reason: String,
    pub(crate) suggestions: Vec<String>,
}

pub fn check_obi_available(obi_path: &str) -> bool {
    if obi_path != "obi" && !obi_path.is_empty() {
        // Explicit path — check file exists
        std::path::Path::new(obi_path).exists()
    } else {
        // Check PATH
        which_obi().is_some()
    }
}

fn which_obi() -> Option<String> {
    which::which("obi")
        .inspect_err(|e| tracing::debug!("obi not found in PATH: {e}"))
        .ok()
        .and_then(|p| camino::Utf8PathBuf::from_path_buf(p).ok())
        .map(camino::Utf8PathBuf::into_string)
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

    #[cfg(target_os = "linux")]
    {
        let capability = match cap {
            "CAP_BPF" => Capability::CAP_BPF,
            "CAP_NET_ADMIN" => Capability::CAP_NET_ADMIN,
            "CAP_SYS_ADMIN" => Capability::CAP_SYS_ADMIN,
            _ => return false,
        };
        caps::has_cap(None, CapSet::Effective, capability).unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = cap;
        false
    }
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
