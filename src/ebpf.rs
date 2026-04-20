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
struct SupervisorHandle {
    supervisor: Supervisor,
    cleanup_path: Option<String>,
}

impl Drop for SupervisorHandle {
    fn drop(&mut self) {
        if let Some(ref path) = self.cleanup_path {
            let _ = std::fs::remove_file(path);
            debug!("cleaned up config file: {}", path);
        }
    }
}

/// Static registry mapping supervisor names to their handles.
static SUPERVISOR_REGISTRY: OnceLock<Mutex<HashMap<String, SupervisorHandle>>> = OnceLock::new();

pub fn get_registry() -> &'static Mutex<HashMap<String, SupervisorHandle>> {
    SUPERVISOR_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_supervisor(name: &str, supervisor: Supervisor, cleanup_path: Option<String>) {
    let registry = get_registry();
    let mut guard = registry.lock().unwrap();
    let handle = SupervisorHandle { supervisor, cleanup_path };
    guard.insert(name.to_string(), handle);
    info!("registered supervisor: {name}");
}

/// Remove any stale OBI config temp files left behind by previous runs.
/// Called at startup to avoid orphaned `/tmp/sigiro-obi-*.yaml` files.
pub fn cleanup_stale_temp_files() {
    let pattern = "/tmp/sigiro-obi-*.yaml";
    let matches = match glob::glob(pattern) {
        Ok(m) => m,
        Err(e) => {
            debug!("glob error scanning for stale temp files: {e}");
            return;
        }
    };
    let count = matches.filter_map(|p| p.ok()).filter_map(|p| std::fs::remove_file(&p).ok()).count();
    if count > 0 {
        info!("cleaned up {count} stale OBI temp file(s)");
    }
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
// Auto-download: obi binary from GitHub releases
// ---------------------------------------------------------------------------

/// Resolves the destination path for the obi binary.
/// Prefers `$SIGIRO_DATA_DIR/bin/obi` if SIGIRO_DATA_DIR is set,
/// otherwise falls back to `~/.local/bin/obi`.
fn obi_install_path() -> std::path::PathBuf {
    if let Ok(data_dir) = std::env::var("SIGIRO_DATA_DIR") {
        let path = std::path::Path::new(&data_dir).join("bin").join("obi");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        return path;
    }
    let home = dirs::home_dir().expect("cannot determine home directory");
    let path = home.join(".local").join("bin").join("obi");
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    path
}

/// One-time install guard — set to Some(path) once obi is successfully installed.
static OBI_INSTALL_TRACKER: OnceLock<String> = OnceLock::new();

/// Ensure obi binary is available on disk, downloading it from GitHub releases
/// if it is not found in PATH and has not already been installed by this function.
///
/// Downloads to `$SIGIRO_DATA_DIR/bin/obi` if `SIGIRO_DATA_DIR` is set,
/// otherwise `~/.local/bin/obi`. The download is performed exactly once
/// per process lifetime using a static `OnceLock`.
pub fn ensure_obi_installed() -> Result<()> {
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
        debug!("obi binary already exists at {}, skipping download", dest.display());
        let _ = OBI_INSTALL_TRACKER.set(dest.to_string_lossy().into_owned());
        return Ok(());
    }

    info!("obi not found in PATH — downloading from GitHub releases to {}", dest.display());

    // Detect target triple for the release asset
    let target = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "aarch64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else {
        anyhow::bail!(
            "obi auto-download is not supported for this platform ({}/{}); \
             please install obi manually",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
    };

    let version = "v0.5.5"
        .trim_start_matches('v');
    let asset_name = format!("obi-{target}");
    let download_url = format!(
        "https://github.com/obi-collector/obi/releases/download/v{}/{}.tar.gz",
        version, asset_name
    );

    // Use reqwest via a single-threaded Tokio runtime (sync context)
    let bytes = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime for obi download")?
        .block_on(async {
            reqwest::get(&download_url)
                .await
                .with_context(|| format!("failed to fetch {}", download_url))?
                .error_for_status()
                .with_context(|| format!("GitHub release returned error for {}", download_url))?
                .bytes()
                .await
                .context("failed to read obi release bytes")
        })?;

    // Decode the .tar.gz archive in-memory then write the inner binary
    let mut decoder = flate2::read::GzDecoder::new(&bytes[..]);
    let mut archive = tar::Archive::new(&mut decoder);
    let mut entries = archive.entries()
        .context("failed to read obi tar archive")?;
    if let Some(Ok(mut entry)) = entries.next() {
        entry.unpack(&dest)
            .with_context(|| format!("failed to extract obi to {}", dest.display()))?;
    } else {
        anyhow::bail!("obi tar archive was empty");
    }

    // Make the binary executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
            .context("failed to set executable permission on obi")?;
    }

    info!("obi successfully installed to {}", dest.display());
    let _ = OBI_INSTALL_TRACKER.set(dest.to_string_lossy().into_owned());
    Ok(())
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
    use uuid::Uuid;

    // Auto-download obi from GitHub releases if not already available
    ensure_obi_installed()?;

    let path = if obi_path.is_empty() { "obi" } else { obi_path };
    info!("starting obi with OTLP endpoint: {otlp_endpoint}");

    let config_uuid = Uuid::new_v4();
    let config_path = format!("/tmp/sigiro-obi-{config_uuid}.yaml");

    // OBI config YAML — mirrors the environment-variable setup used in k8s
    let config_yaml = format!(
        r#"otel_exporter_otlp_endpoint: "{otlp_endpoint}"
otel_ebpf_auto_target_exe: "*"
"#,
    );
    std::fs::write(&config_path, config_yaml)
        .context("failed to write obi config file")?;

    let mut envs = HashMap::new();
    envs.insert("OTEL_EXPORTER_OTLP_ENDPOINT".to_string(), otlp_endpoint.to_string());
    // Pass config path via environment variable (OBI reads this or we pass as arg)
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
