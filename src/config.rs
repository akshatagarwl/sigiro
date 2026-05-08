use anyhow::{Context, Result};
use std::path::PathBuf;
use typed_env::{Envar, EnvarDef};

// ---------------------------------------------------------------------------
// Environment variable definitions
// ---------------------------------------------------------------------------
// All env vars are declared here as typed statics. Benefits:
// - Fail-fast on malformed values (e.g., SIGIRO_HTTP_PORT=garbage → error)
// - Clear error messages with var name, type, and bad value
// - Defaults are explicit and centralized
// ---------------------------------------------------------------------------

/// Server ports. Default to well-known OTLP ports when unset.
pub static HTTP_PORT: Envar<u16> =
    Envar::on_startup("SIGIRO_HTTP_PORT", || EnvarDef::Default(9999));
pub static GRPC_PORT: Envar<u16> =
    Envar::on_startup("SIGIRO_GRPC_PORT", || EnvarDef::Default(4317));
pub static OTLP_HTTP_PORT: Envar<u16> =
    Envar::on_startup("SIGIRO_OTLP_HTTP_PORT", || EnvarDef::Default(4318));

/// S3 cold storage config. Empty/unset means no S3.
pub static S3_BUCKET: Envar<String> = Envar::on_startup("SIGIRO_S3_BUCKET", || EnvarDef::Unset);
pub static S3_REGION: Envar<String> =
    Envar::on_startup("SIGIRO_S3_REGION", || EnvarDef::Default(String::new()));
pub static S3_PREFIX: Envar<String> = Envar::on_startup("SIGIRO_S3_PREFIX", || EnvarDef::Unset);

/// OTel self-instrumentation endpoint. Defaults to local sigiro OTLP HTTP endpoint.
pub static OTEL_ENDPOINT: Envar<String> = Envar::on_startup("SIGIRO_OTEL_ENDPOINT", || {
    EnvarDef::Default("http://127.0.0.1:4318".to_string())
});

/// OTel service name for self-instrumentation.
pub static OTEL_SERVICE_NAME: Envar<String> = Envar::on_startup("OTEL_SERVICE_NAME", || {
    EnvarDef::Default("sigiro".to_string())
});

/// OTel OTLP exporter endpoint (for self-instrumentation traces).
pub static OTEL_EXPORTER_OTLP_ENDPOINT: Envar<String> =
    Envar::on_startup("OTEL_EXPORTER_OTLP_ENDPOINT", || {
        EnvarDef::Default(String::new())
    });

/// OTel auth key for self-ingestion (optional). Empty means no auth.
pub static OTEL_AUTH_KEY: Envar<String> =
    Envar::on_startup("SIGIRO_OTEL_AUTH_KEY", || EnvarDef::Default(String::new()));
/// Pyroscope profiling endpoint (optional — no profiling if unset).
pub static PYROSCOPE_ENDPOINT: Envar<String> =
    Envar::on_startup("SIGIRO_PYROSCOPE_ENDPOINT", || {
        EnvarDef::Default(String::new())
    });

/// Flush interval for per-project flusher tasks (seconds). Default: 1s.
/// Controls how often buffered rows are flushed to DuckDB, even if the row
/// threshold hasn't been reached. Lower values reduce data-loss window on
/// crashes; higher values reduce I/O overhead.
pub static FLUSH_INTERVAL_SECS: Envar<u64> =
    Envar::on_startup("SIGIRO_FLUSH_INTERVAL_SECS", || EnvarDef::Default(1));

/// Row count threshold that triggers an immediate flush. Default: 10_000.
/// When this many rows are buffered across all signal tables, the flusher
/// writes them to DuckDB without waiting for the timer.
pub static FLUSH_ROW_THRESHOLD: Envar<usize> =
    Envar::on_startup("SIGIRO_FLUSH_ROW_THRESHOLD", || EnvarDef::Default(10_000));

/// Backpressure policy when the per-project channel is full. Default: "block".
/// - "block": sender awaits space (preserves data, may slow ingest)
/// - "drop-oldest": drops the oldest buffered message (prevents stalls, may lose data)
pub static BACKPRESSURE_POLICY: Envar<String> =
    Envar::on_startup("SIGIRO_BACKPRESSURE_POLICY", || {
        EnvarDef::Default("block".to_string())
    });

/// Data directory for sigiro (holds DuckDB files, extensions, binaries).
/// Path to the sigiro data directory (required — no default). Unset causes fail-fast at startup.
pub static DATA_DIR: Envar<String> = Envar::on_startup("SIGIRO_DATA_DIR", || EnvarDef::Unset);

/// Server URL for CLI client and admin commands.
pub static ENDPOINT: Envar<String> = Envar::on_startup("SIGIRO_ENDPOINT", || {
    EnvarDef::Default("http://localhost:9999".to_string())
});

/// PostgreSQL catalog URL (optional — enables distributed mode).
/// Unset or empty means local SQLite. Empty string means misconfiguration (fail-fast).
pub static CATALOG_URL: Envar<String> = Envar::on_startup("SIGIRO_CATALOG_URL", || EnvarDef::Unset);

/// Machine hostname (used for federation worker registration).
pub static HOSTNAME: Envar<String> =
    Envar::on_startup("HOSTNAME", || EnvarDef::Default(String::new()));

/// API key (required — no default). Env takes priority over config.toml.
/// Uses EnvarDef::Unset so .value() fails if not set (no silent fallback).
pub static API_KEY: Envar<String> = Envar::on_startup("SIGIRO_API_KEY", || EnvarDef::Unset);

/// API key (required — no default. Set via $SIGIRO_API_KEY or config.toml).
/// Returns the path to the sigiro config directory (~/.config/sigiro/).
fn config_dir() -> Result<PathBuf> {
    let base = dirs::config_dir().context("cannot determine config directory")?;
    Ok(base.join("sigiro"))
}

/// Returns the path to config.toml.
fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

/// Resolve API key: env var takes priority, then config.toml.
pub fn get_api_key() -> Result<String> {
    if let Ok(key) = crate::config::API_KEY.value() {
        return Ok(key);
    }
    let path = config_path()?;
    let content = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "No API key found. Run: sigiro auth <key>\n  (looked in {} and $SIGIRO_API_KEY)",
            path.display()
        )
    })?;
    let config: toml::Value = content.parse().context("invalid config.toml")?;
    config["api_key"]
        .as_str()
        .map(String::from)
        .context("api_key not found in config.toml")
}

/// Save API key to ~/.config/sigiro/config.toml with 0600 permissions on Unix.
pub fn save_api_key(key: &str) -> Result<PathBuf> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create config dir: {}", dir.display()))?;
    let path = dir.join("config.toml");
    std::fs::write(&path, format!("api_key = \"{key}\"\n"))
        .with_context(|| format!("failed to write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .context("failed to set config.toml permissions to 0600")?;
    }
    Ok(path)
}
