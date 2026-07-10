use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
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
/// on_demand (not on_startup) so temp_env::with_vars works in tests.
pub static S3_BUCKET: Envar<String> =
    Envar::on_demand("SIGIRO_S3_BUCKET", || EnvarDef::Default(String::new()));
pub static S3_REGION: Envar<String> =
    Envar::on_startup("SIGIRO_S3_REGION", || EnvarDef::Default(String::new()));
/// S3-compatible endpoint host (no scheme), e.g. `<account>.r2.cloudflarestorage.com`
/// for Cloudflare R2. Required when `SIGIRO_S3_BUCKET` is set — R2/most S3-compatible
/// stores are unreachable via DuckDB's default AWS endpoint resolution.
/// on_demand (not on_startup) so temp_env::with_vars works in tests.
pub static S3_ENDPOINT: Envar<String> =
    Envar::on_demand("SIGIRO_S3_ENDPOINT", || EnvarDef::Default(String::new()));
/// S3 access key ID. Falls back to the conventional `AWS_ACCESS_KEY_ID` when unset.
/// on_demand (not on_startup) so temp_env::with_vars works in tests.
pub static S3_KEY_ID: Envar<String> =
    Envar::on_demand("SIGIRO_S3_KEY_ID", || EnvarDef::Default(String::new()));
/// S3 secret access key. Falls back to the conventional `AWS_SECRET_ACCESS_KEY` when unset.
/// on_demand (not on_startup) so temp_env::with_vars works in tests.
pub static S3_SECRET: Envar<String> =
    Envar::on_demand("SIGIRO_S3_SECRET", || EnvarDef::Default(String::new()));
/// Conventional AWS credential env vars — fallback source for S3_KEY_ID/S3_SECRET.
/// on_demand (not on_startup) so temp_env::with_vars works in tests.
pub static AWS_ACCESS_KEY_ID: Envar<String> =
    Envar::on_demand("AWS_ACCESS_KEY_ID", || EnvarDef::Default(String::new()));
pub static AWS_SECRET_ACCESS_KEY: Envar<String> =
    Envar::on_demand("AWS_SECRET_ACCESS_KEY", || EnvarDef::Default(String::new()));

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
/// DuckDB extension directory (optional — shared cache across tests).
pub static EXTENSION_DIR: Envar<String> =
    Envar::on_startup("SIGIRO_EXTENSION_DIR", || EnvarDef::Default(String::new()));

/// DuckDB worker threads per shared project database. Default: host parallelism.
pub static DUCKDB_THREADS: Envar<usize> = Envar::on_startup("SIGIRO_DUCKDB_THREADS", || {
    EnvarDef::Default(default_duckdb_threads())
});

/// DuckDB memory cap per shared project database. Default: 400 MB — safely
/// under the smallest hosted VM (fly.toml provisions 512 MB), so DuckDB
/// errors/spills instead of the OS OOM-killing the whole process. Still
/// env-overridable for larger boxes.
pub static DUCKDB_MEMORY_LIMIT: Envar<String> =
    Envar::on_startup("SIGIRO_DUCKDB_MEMORY_LIMIT", || {
        EnvarDef::Default("400MB".to_string())
    });

/// Per-query wall-clock timeout in seconds. DuckDB (bundled version) has no
/// `statement_timeout` setting, so this is enforced by interrupting the
/// reader connection from a watchdog once the deadline passes (see
/// `execute_sql` in server.rs). Default: 30s.
pub static QUERY_TIMEOUT_SECONDS: Envar<u64> =
    Envar::on_startup("SIGIRO_QUERY_TIMEOUT_SECONDS", || EnvarDef::Default(30));

/// Max rows a single `/v1/query` call will collect before stopping early and
/// reporting truncation. Default: 100_000 — bounds the in-memory result Vec
/// and the JSON response size for a hosted multi-tenant instance.
pub static MAX_RESULT_ROWS: Envar<usize> =
    Envar::on_startup("SIGIRO_MAX_RESULT_ROWS", || EnvarDef::Default(100_000));

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
/// - "drop-newest": drops the incoming message (prevents stalls, loses newest data)
pub static BACKPRESSURE_POLICY: Envar<String> =
    Envar::on_startup("SIGIRO_BACKPRESSURE_POLICY", || {
        EnvarDef::Default("block".to_string())
    });

/// Backpressure policy for per-project ingest channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackpressurePolicy {
    /// Block the sender until space is available. Guarantees no data loss but may
    /// cause backpressure on the OTLP client.
    Block,
    /// Drop the incoming message when the channel is full.
    DropNewest,
}

pub fn backpressure_policy() -> Result<BackpressurePolicy> {
    match BACKPRESSURE_POLICY.value()?.as_str() {
        "block" => Ok(BackpressurePolicy::Block),
        "drop-newest" => Ok(BackpressurePolicy::DropNewest),
        other => anyhow::bail!(
            "unknown SIGIRO_BACKPRESSURE_POLICY: {other}. Expected 'block' or 'drop-newest'"
        ),
    }
}

pub fn duckdb_threads() -> Result<usize> {
    let threads = DUCKDB_THREADS.value()?;
    anyhow::ensure!(
        threads > 0,
        "SIGIRO_DUCKDB_THREADS must be greater than zero"
    );
    Ok(threads)
}

pub fn duckdb_memory_limit() -> Result<String> {
    let memory_limit = DUCKDB_MEMORY_LIMIT.value()?;
    anyhow::ensure!(
        !memory_limit.trim().is_empty(),
        "SIGIRO_DUCKDB_MEMORY_LIMIT must not be empty"
    );
    Ok(memory_limit)
}

pub fn query_timeout_secs() -> Result<u64> {
    let secs = QUERY_TIMEOUT_SECONDS.value()?;
    anyhow::ensure!(
        secs > 0,
        "SIGIRO_QUERY_TIMEOUT_SECONDS must be greater than zero"
    );
    Ok(secs)
}

pub fn max_result_rows() -> Result<usize> {
    let rows = MAX_RESULT_ROWS.value()?;
    anyhow::ensure!(rows > 0, "SIGIRO_MAX_RESULT_ROWS must be greater than zero");
    Ok(rows)
}

fn default_duckdb_threads() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
}

/// Byte-size threshold that triggers an immediate flush. Default: 8_388_608 (8 MiB).
/// When the total buffered bytes across all signal tables reaches this limit,
/// the flusher writes to DuckDB without waiting for the row threshold or timer.
/// This prevents unbounded memory growth when individual rows are large.
pub static FLUSH_BYTE_THRESHOLD: Envar<usize> =
    Envar::on_startup("SIGIRO_FLUSH_BYTE_THRESHOLD", || {
        EnvarDef::Default(8_388_608)
    });

/// Max ingest request body size in bytes (OTLP HTTP `DefaultBodyLimit`, and
/// the matching gRPC `max_decoding_message_size` — see `crate::ingest`).
/// Default: 8_388_608 (8 MiB). The HTTP cap applies to the DECOMPRESSED size
/// (enforced when the body is collected into `Bytes`, which happens after
/// `RequestDecompressionLayer` has already un-gzipped it), so this also
/// bounds a gzip-bomb, not just the wire size.
pub static MAX_INGEST_BODY_BYTES: Envar<usize> =
    Envar::on_startup("SIGIRO_MAX_INGEST_BODY_BYTES", || {
        EnvarDef::Default(8_388_608)
    });

pub fn max_ingest_body_bytes() -> Result<usize> {
    let bytes = MAX_INGEST_BODY_BYTES.value()?;
    anyhow::ensure!(
        bytes > 0,
        "SIGIRO_MAX_INGEST_BODY_BYTES must be greater than zero"
    );
    Ok(bytes)
}

/// Data directory for sigiro (holds DuckDB files, extensions, binaries).
/// Defaults to the platform data dir (~/.local/share/sigiro on Unix, %LOCALAPPDATA%\sigiro on Windows).
pub static DATA_DIR: Envar<String> = Envar::on_startup("SIGIRO_DATA_DIR", || EnvarDef::Unset);

/// Server URL for CLI client and admin commands.
pub static ENDPOINT: Envar<String> = Envar::on_startup("SIGIRO_ENDPOINT", || {
    EnvarDef::Default("http://localhost:9999".to_string())
});

/// Path to TLS certificate PEM file. Unset = no TLS.
pub static TLS_CERT: Envar<String> = Envar::on_startup("SIGIRO_TLS_CERT", || EnvarDef::Unset);
/// Path to TLS private key PEM file. Unset = no TLS.
pub static TLS_KEY: Envar<String> = Envar::on_startup("SIGIRO_TLS_KEY", || EnvarDef::Unset);

/// Data retention days. 0 = no automatic cleanup.
pub static RETENTION_DAYS: Envar<u32> =
    Envar::on_startup("SIGIRO_RETENTION_DAYS", || EnvarDef::Default(0));

/// PostgreSQL catalog URL (required for distributed/multi-tenant mode, optional
/// otherwise). Unset means local SQLite catalog.
/// Deployment modes: docs/architecture/deployment-modes.html
pub static CATALOG_URL: Envar<String> = Envar::on_startup("SIGIRO_CATALOG_URL", || EnvarDef::Unset);

/// Which cross-project query surface (if any) this node exposes.
/// - "none" (default): no cross-project query surface.
/// - "quack": the DuckDB `quack` extension serves this node's DuckLake data
///   for external ATTACH. Inert until the query-surface feature lands.
pub static QUERY_SURFACE: Envar<String> = Envar::on_startup("SIGIRO_QUERY_SURFACE", || {
    EnvarDef::Default("none".to_string())
});

/// Parsed form of `QUERY_SURFACE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuerySurface {
    None,
    Quack,
}

pub fn query_surface() -> Result<QuerySurface> {
    match QUERY_SURFACE.value()?.as_str() {
        "none" => Ok(QuerySurface::None),
        "quack" => Ok(QuerySurface::Quack),
        other => anyhow::bail!("unknown SIGIRO_QUERY_SURFACE: {other}. Expected 'none' or 'quack'"),
    }
}

/// Bind address for the quack read-only query-serving surface (only used
/// when `SIGIRO_QUERY_SURFACE=quack`). NEVER default to `0.0.0.0` — quack
/// speaks plain HTTP with no working TLS of its own, so binding every
/// interface by default would put an unauthenticated-by-default protocol
/// straight on the open network. Operators opt into a wider bind explicitly
/// (e.g. a Tailscale interface IP) and own the network perimeter themselves.
pub static QUACK_BIND: Envar<String> = Envar::on_startup("SIGIRO_QUACK_BIND", || {
    EnvarDef::Default("127.0.0.1".to_string())
});

/// Port for the quack query-serving surface.
pub static QUACK_PORT: Envar<u16> =
    Envar::on_startup("SIGIRO_QUACK_PORT", || EnvarDef::Default(7777));

/// Optional bearer token quack clients must present. Empty (the default)
/// means no token — an acceptable choice only because the bind address
/// above defaults to loopback and any wider exposure is the operator's own,
/// explicit choice. on_demand (not on_startup) so temp_env::with_vars works
/// in tests, matching the S3_* secret pattern above.
pub static QUACK_TOKEN: Envar<String> =
    Envar::on_demand("SIGIRO_QUACK_TOKEN", || EnvarDef::Default(String::new()));

/// API key (required — no default). Env takes priority over config.toml.
/// Uses EnvarDef::Unset so .value() fails if not set (no silent fallback).
pub static API_KEY: Envar<String> = Envar::on_startup("SIGIRO_API_KEY", || EnvarDef::Unset);

/// Admin bearer token for distributed tenant/key provisioning endpoints.
/// Unset disables those mutation endpoints.
pub static ADMIN_TOKEN: Envar<String> = Envar::on_startup("SIGIRO_ADMIN_TOKEN", || EnvarDef::Unset);

#[derive(Deserialize, Serialize)]
struct ConfigFile {
    api_key: String,
}

/// API key (required — no default. Set via $SIGIRO_API_KEY or config.toml).
/// Returns the path to the sigiro config directory (~/.config/sigiro/).
fn config_dir() -> Result<Utf8PathBuf> {
    let base = dirs::config_dir().context("cannot determine config directory")?;
    let base = Utf8PathBuf::from_path_buf(base)
        .map_err(|_| anyhow::anyhow!("config directory path is not valid UTF-8"))?;
    Ok(base.join("sigiro"))
}

/// Returns the path to config.toml.
fn config_path() -> Result<Utf8PathBuf> {
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
            "No API key found. Run: sigiro auth <key>\n  (looked in {path} and $SIGIRO_API_KEY)",
        )
    })?;
    let config: ConfigFile = toml::from_str(&content).context("invalid config.toml")?;
    Ok(config.api_key)
}

/// Save API key to ~/.config/sigiro/config.toml with 0600 permissions on Unix.
pub fn save_api_key(key: &str) -> Result<Utf8PathBuf> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create config dir: {dir}"))?;
    let path = dir.join("config.toml");
    let content = toml::to_string(&ConfigFile {
        api_key: key.to_string(),
    })
    .context("failed to serialize config.toml")?;
    std::fs::write(&path, content).with_context(|| format!("failed to write {path}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .context("failed to set config.toml permissions to 0600")?;
    }
    Ok(path)
}
