use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tracing::{debug, info};

use crate::supervisor::Supervisor;

// ---------------------------------------------------------------------------
// Supervisor registry — keeps supervisor handles alive for the process lifetime
// ---------------------------------------------------------------------------

/// Handle wrapping a supervisor and optional cleanup path.
/// The cleanup runs on drop to remove temp config files.
pub struct SupervisorHandle {
    #[allow(dead_code)]
    supervisor: Supervisor, // intentionally held to keep the job alive
    cleanup_path: Option<String>,
}

impl Drop for SupervisorHandle {
    fn drop(&mut self) {
        if let Some(ref path) = self.cleanup_path {
            let _ = std::fs::remove_file(path).ok();
            debug!("cleaned up config file: {}", path);
        }
    }
}

/// Static registry mapping supervisor names to their handles.
static SUPERVISOR_REGISTRY: OnceLock<Mutex<HashMap<String, SupervisorHandle>>> = OnceLock::new();

fn get_registry() -> &'static Mutex<HashMap<String, SupervisorHandle>> {
    SUPERVISOR_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
mod test_utils {
    use super::get_registry;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Accessor for tests — avoids exposing SupervisorHandle publicly.
    pub fn with_registry<F, R>(f: F) -> R
    where
        F: FnOnce(&Mutex<HashMap<String, super::SupervisorHandle>>) -> R,
    {
        f(get_registry())
    }
}

fn register_supervisor(name: &str, supervisor: Supervisor, cleanup_path: Option<String>) {
    let registry = get_registry();
    let mut guard = registry.lock().unwrap();
    let handle = SupervisorHandle {
        supervisor,
        cleanup_path,
    };
    guard.insert(name.to_string(), handle);
    info!("registered supervisor: {name}");
}

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

// ---------------------------------------------------------------------------
// Auto-install: kernel-collector binary from OTel eBPF releases (Linux only)
// ---------------------------------------------------------------------------

/// One-time install guard — set to Some(path) once obi is successfully installed.
#[allow(dead_code)]
static OBI_INSTALL_TRACKER: OnceLock<String> = OnceLock::new();

/// Resolves the destination path for the obi binary.
/// Prefers `$SIGIRO_DATA_DIR/bin/obi` if SIGIRO_DATA_DIR is set,
/// otherwise falls back to `~/.local/bin/obi`.
#[allow(dead_code)]
fn obi_install_path() -> std::path::PathBuf {
    if let Ok(data_dir) = std::env::var("SIGIRO_DATA_DIR") {
        let path = std::path::Path::new(&data_dir).join("bin").join("obi");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent).ok();
        }
        return path;
    }
    let home = dirs::home_dir().expect("cannot determine home directory");
    let path = home.join(".local").join("bin").join("obi");
    let _ = std::fs::create_dir_all(path.parent().unwrap()).ok();
    path
}

/// Download and extract the OTel eBPF kernel-collector binary.
/// The release ships as a .deb archive; we extract using `ar` (no dpkg needed).
#[allow(dead_code)]
fn download_kernel_collector(dest: &std::path::Path) -> Result<()> {
    let version = "v0.11.0";

    // Detect target arch for the release asset
    let asset_name = if cfg!(target_arch = "x86_64") {
        format!(
            "opentelemetry-ebpf-kernel-collector_{}_amd64.deb",
            version.trim_start_matches('v')
        )
    } else {
        anyhow::bail!(
            "no pre-built kernel-collector deb for {}/{}; \
             install manually or use the .rpm on CentOS/RHEL: \
             https://github.com/open-telemetry/opentelemetry-ebpf/releases/{}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            version
        );
    };
    let download_url = format!(
        "https://github.com/open-telemetry/opentelemetry-network/releases/download/{}/{}",
        version, asset_name
    );

    info!("downloading {}", download_url);

    // Download the .deb archive
    let bytes = reqwest::blocking::get(&download_url)
        .with_context(|| format!("failed to fetch {}", download_url))?
        .error_for_status()
        .with_context(|| format!("GitHub release returned error for {}", download_url))?
        .bytes()
        .with_context(|| "failed to read kernel-collector deb bytes")?;

    // .deb format (ar archive): magic "!<arch>\n" at offset 0
    if bytes.len() < 8 || &bytes[0..8] != b"!<arch>\n" {
        anyhow::bail!("downloaded file is not a valid deb archive");
    }

    // Extract data.tar.gz from the ar archive
    let data_tar_gz = extract_ar_entry(&bytes, "data.tar.gz")
        .context("failed to find data.tar.gz inside deb archive")?;

    // Decompress and extract the binary from data.tar.gz
    extract_tar_binary(&data_tar_gz, "usr/bin/kernel-collector", dest)
        .context("failed to extract kernel-collector binary from data.tar.gz")?;

    info!("kernel-collector extracted to {}", dest.display());
    Ok(())
}

/// Extract a named entry from an ar archive (e.g. a .deb).
/// Returns the raw contents of the entry.
#[allow(dead_code)]
fn extract_ar_entry(ar_data: &[u8], entry_name: &str) -> Option<Vec<u8>> {
    // ar archive format:
    //   8-byte magic "!<arch>\n"
    //   then member headers (60 bytes each):
    //     0-16:  file name (space-padded to 16 chars)
    //    16-28:  modification timestamp (ascii)
    //    28-34:  owner ID (ascii)
    //    34-40:  group ID (ascii)
    //    40-48:  file mode (octal ascii)
    //    48-58:  size (decimal ascii, 10 chars)
    //    58-60:  magic "\x60\x0a"
    //   then file data (padded to 2-byte boundary)
    let mut offset = 8;
    while offset + 60 <= ar_data.len() {
        let name = std::str::from_utf8(&ar_data[offset..offset + 16])
            .ok()?
            .trim_end_matches(' ');
        let size_str = std::str::from_utf8(&ar_data[offset + 48..offset + 58]).ok()?;
        let size: usize = size_str.trim_end_matches(' ').parse().ok()?;
        offset += 60; // skip header

        if name == entry_name {
            return Some(ar_data[offset..offset + size].to_vec());
        }

        // ar entries are aligned to 2-byte boundaries
        offset += size;
        if offset % 2 != 0 {
            offset += 1;
        }
    }
    None
}

/// Extract a single file from a gzip-compressed tar archive.
#[allow(dead_code)]
fn extract_tar_binary(gz_data: &[u8], entry_path: &str, dest: &std::path::Path) -> Result<()> {
    let mut decoder = flate2::read::GzDecoder::new(gz_data);
    let mut archive = tar::Archive::new(&mut decoder);

    for entry in archive.entries().context("failed to read tar archive")? {
        let mut entry = entry.context("failed to read tar entry")?;
        let entry_path_str = entry
            .path()
            .context("entry has no path")?
            .to_string_lossy()
            .into_owned();

        if entry_path_str == entry_path {
            entry.unpack(dest).with_context(|| {
                format!("failed to extract {} to {}", entry_path, dest.display())
            })?;
            return Ok(());
        }
    }

    anyhow::bail!("{} not found in tar archive", entry_path);
}

/// Ensure obi binary is available on disk, downloading from OTel eBPF releases
/// if it is not found in PATH and has not already been installed by this function.
///
/// Downloads the `kernel-collector` binary from `open-telemetry/opentelemetry-ebpf`
/// releases (as a `.deb` archive — extracted without dpkg).
///
/// Downloads to `$SIGIRO_DATA_DIR/bin/obi` if `SIGIRO_DATA_DIR` is set,
/// otherwise `~/.local/bin/obi`. The download is performed exactly once
/// per process lifetime using a static `OnceLock`.
pub fn ensure_obi_installed() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        ensure_obi_installed_linux()
    }
    #[cfg(not(target_os = "linux"))]
    {
        ensure_obi_installed_other()
    }
}

#[cfg(target_os = "linux")]
fn ensure_obi_installed_linux() -> Result<()> {
    // Fast path: already resolved this call
    if OBI_INSTALL_TRACKER.get().is_some() {
        return Ok(());
    }

    // Check if obi is already in PATH
    if which_obi().is_some() {
        debug!("obi found in PATH, no download needed");
        return Ok(());
    }

    let dest = obi_install_path();
    if dest.exists() {
        debug!(
            "obi binary already exists at {}, skipping download",
            dest.display()
        );
        let _ = OBI_INSTALL_TRACKER
            .set(dest.to_string_lossy().into_owned())
            .ok();
        return Ok(());
    }

    info!(
        "obi not found in PATH — downloading OTel eBPF kernel-collector to {}",
        dest.display()
    );

    download_kernel_collector(&dest)?;

    // Make the binary executable
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
            .context("failed to set executable permission on obi")?;
    }

    info!("obi successfully installed to {}", dest.display());
    let _ = OBI_INSTALL_TRACKER
        .set(dest.to_string_lossy().into_owned())
        .ok();
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn ensure_obi_installed_other() -> Result<()> {
    // Check if obi is in PATH even on non-Linux
    if which_obi().is_some() {
        debug!("obi found in PATH, no download needed");
        return Ok(());
    }

    anyhow::bail!(
        "obi auto-download is not supported on {}/{}; \
         please install the OpenTelemetry eBPF kernel-collector manually: \
         https://github.com/open-telemetry/opentelemetry-ebpf",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
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
/// Returns Err if obi failed to spawn.
pub fn start_obi(obi_path: &str, otlp_endpoint: &str) -> Result<()> {
    use uuid::Uuid;

    // Auto-download OTel eBPF kernel-collector if not already available (Linux only)
    ensure_obi_installed()?;

    let path = if obi_path.is_empty() { "obi" } else { obi_path };
    info!("starting obi with OTLP endpoint: {otlp_endpoint}");

    let config_uuid = Uuid::new_v4();
    let config_path = format!("/tmp/sigiro-obi-{config_uuid}.yaml");

    // OBI config YAML
    let config_yaml = format!(
        r#"otel_exporter_otlp_endpoint: "{otlp_endpoint}"
otel_ebpf_auto_target_exe: "*"
"#,
    );
    std::fs::write(&config_path, config_yaml).context("failed to write obi config file")?;

    let mut envs = HashMap::new();
    envs.insert(
        "OTEL_EXPORTER_OTLP_ENDPOINT".to_string(),
        otlp_endpoint.to_string(),
    );
    envs.insert("OTEL_EBPF_CONFIG".to_string(), config_path.clone());

    let supervisor = Supervisor::new(path, vec![format!("--config={}", config_path)], envs);
    supervisor.start();

    register_supervisor("obi", supervisor, Some(config_path));

    info!("obi started via supervisor");
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
