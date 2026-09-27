use anyhow::{Context, Result};
use std::num::{NonZeroU64, NonZeroUsize};
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
pub(crate) static HTTP_PORT: Envar<u16> =
    Envar::on_startup("SIGIRO_HTTP_PORT", || EnvarDef::Default(9999));
pub(crate) static GRPC_PORT: Envar<u16> =
    Envar::on_startup("SIGIRO_GRPC_PORT", || EnvarDef::Default(4317));
pub(crate) static OTLP_HTTP_PORT: Envar<u16> =
    Envar::on_startup("SIGIRO_OTLP_HTTP_PORT", || EnvarDef::Default(4318));

/// S3 cold storage config. Empty/unset means no S3.
pub(crate) static S3_BUCKET: Envar<String> =
    Envar::on_demand("SIGIRO_S3_BUCKET", || EnvarDef::Default(String::new()));
pub(crate) static S3_REGION: Envar<String> =
    Envar::on_startup("SIGIRO_S3_REGION", || EnvarDef::Default(String::new()));
/// Absolute HTTP(S) endpoint URL, e.g.
/// `https://<account>.r2.cloudflarestorage.com` for Cloudflare R2. Required when
/// `SIGIRO_S3_BUCKET` is set — R2/most S3-compatible stores are unreachable via
/// DuckDB's default AWS endpoint resolution. The scheme determines DuckDB's
/// `USE_SSL` setting.
pub(crate) static S3_ENDPOINT: Envar<String> =
    Envar::on_demand("SIGIRO_S3_ENDPOINT", || EnvarDef::Default(String::new()));
/// S3 access key ID. Falls back to the conventional `AWS_ACCESS_KEY_ID` when unset.
pub(crate) static S3_KEY_ID: Envar<String> =
    Envar::on_demand("SIGIRO_S3_KEY_ID", || EnvarDef::Default(String::new()));
/// S3 secret access key. Falls back to the conventional `AWS_SECRET_ACCESS_KEY` when unset.
pub(crate) static S3_SECRET: Envar<String> =
    Envar::on_demand("SIGIRO_S3_SECRET", || EnvarDef::Default(String::new()));
/// Conventional AWS credential env vars — fallback source for S3_KEY_ID/S3_SECRET.
pub(crate) static AWS_ACCESS_KEY_ID: Envar<String> =
    Envar::on_demand("AWS_ACCESS_KEY_ID", || EnvarDef::Default(String::new()));
pub(crate) static AWS_SECRET_ACCESS_KEY: Envar<String> =
    Envar::on_demand("AWS_SECRET_ACCESS_KEY", || EnvarDef::Default(String::new()));

/// OTel service name for self-instrumentation.
pub(crate) static OTEL_SERVICE_NAME: Envar<String> = Envar::on_startup("OTEL_SERVICE_NAME", || {
    EnvarDef::Default("sigiro".to_string())
});

/// OTel OTLP exporter endpoint (for self-instrumentation traces).
pub(crate) static OTEL_EXPORTER_OTLP_ENDPOINT: Envar<String> =
    Envar::on_startup("OTEL_EXPORTER_OTLP_ENDPOINT", || {
        EnvarDef::Default(String::new())
    });

/// OTel auth key for self-ingestion (optional). Empty means no auth.
pub(crate) static OTEL_AUTH_KEY: Envar<String> =
    Envar::on_startup("SIGIRO_OTEL_AUTH_KEY", || EnvarDef::Default(String::new()));

/// Deployment-wide Alertmanager-compatible webhook. Empty disables delivery.
pub(crate) static ALERT_WEBHOOK_URL: Envar<String> =
    Envar::on_startup("SIGIRO_ALERT_WEBHOOK_URL", || {
        EnvarDef::Default(String::new())
    });
/// Optional bearer token for the alert webhook.
static ALERT_WEBHOOK_TOKEN: Envar<String> = Envar::on_demand("SIGIRO_ALERT_WEBHOOK_TOKEN", || {
    EnvarDef::Default(String::new())
});

pub(crate) fn alert_webhook_token() -> Result<Option<crate::alerts::AlertWebhookToken>> {
    let raw = ALERT_WEBHOOK_TOKEN.value()?;
    if raw.is_empty() {
        return Ok(None);
    }
    crate::alerts::AlertWebhookToken::try_new(raw)
        .map(Some)
        .map_err(|error| anyhow::anyhow!("invalid SIGIRO_ALERT_WEBHOOK_TOKEN: {error}"))
}
/// Optional DuckDB extension-cache override. Normal deployments use
/// `<SIGIRO_DATA_DIR>/extensions`.
pub(crate) static EXTENSION_DIR: Envar<String> =
    Envar::on_startup("SIGIRO_EXTENSION_DIR", || EnvarDef::Default(String::new()));

/// Allow DuckDB to load extensions without a valid signature.
///
/// DuckDB refuses unsigned `.duckdb_extension` files unless this is set at
/// Config time — it cannot be changed once the database is open. Needed only to
/// run a locally built extension, e.g. carrying an upstream fix that has not
/// been published for the pinned DuckDB version. Defaults to false: signature
/// checking stays on unless an operator opts out deliberately.
pub(crate) static ALLOW_UNSIGNED_EXTENSIONS: Envar<bool> =
    Envar::on_startup("SIGIRO_ALLOW_UNSIGNED_EXTENSIONS", || {
        EnvarDef::Default(false)
    });

/// DuckDB worker threads per shared project database. Default: host parallelism.
static DUCKDB_THREADS: Envar<usize> = Envar::on_startup("SIGIRO_DUCKDB_THREADS", || {
    EnvarDef::Default(
        std::thread::available_parallelism()
            .expect("failed to determine available parallelism")
            .get(),
    )
});

/// DuckDB memory cap per shared project database. Default: 400 MB — safely
/// under a small hosted container's limit, so DuckDB errors/spills instead of
/// the OS OOM-killing the whole process. Still env-overridable for larger boxes.
pub(crate) static DUCKDB_MEMORY_LIMIT: Envar<String> =
    Envar::on_startup("SIGIRO_DUCKDB_MEMORY_LIMIT", || {
        EnvarDef::Default("400MB".to_string())
    });

/// Per-query wall-clock timeout in seconds. DuckDB (pinned version) has no
/// `statement_timeout` setting, so this is enforced by interrupting the
/// reader connection from a watchdog once the deadline passes (see
/// `execute_sql` in server.rs). Default: 30s.
pub(crate) static QUERY_TIMEOUT_SECONDS: Envar<u64> =
    Envar::on_startup("SIGIRO_QUERY_TIMEOUT_SECONDS", || EnvarDef::Default(30));

/// Max rows a single `/v1/query` call will collect before stopping early and
/// reporting truncation. Default: 100_000 — bounds the in-memory result Vec
/// and the JSON response size for a hosted multi-tenant instance.
pub(crate) static MAX_RESULT_ROWS: Envar<usize> =
    Envar::on_startup("SIGIRO_MAX_RESULT_ROWS", || EnvarDef::Default(100_000));

/// Pyroscope profiling endpoint (optional — no profiling if unset).
pub(crate) static PYROSCOPE_ENDPOINT: Envar<String> =
    Envar::on_startup("SIGIRO_PYROSCOPE_ENDPOINT", || {
        EnvarDef::Default(String::new())
    });

/// Flush interval for per-project flusher tasks (seconds). Default: 60s.
/// Controls how often buffered rows are flushed to DuckDB, even if the row
/// threshold hasn't been reached. Lower values reduce data-loss window on
/// crashes; higher values reduce I/O overhead.
///
/// This timer only decides flush cadence for LOW-traffic projects: under load
/// `FLUSH_ROW_THRESHOLD` (10k rows) or `FLUSH_BYTE_THRESHOLD` (8 MiB) fires
/// first, so the buffer stays bounded regardless of this value.
///
/// This was 60 for part of 2026-08-24, on the theory that a 1s timer made quiet
/// projects emit one tiny Parquet file per second. **Measured on production, it
/// did not work.** Over a 15-hour window the file rate went from 612-910/day to
/// 407/day, and average file size moved 11 KiB to 12 KiB. The theory was wrong
/// about the mechanism: at ~0.5 rows/s telemetry arrives in bursts every few
/// minutes and the flusher skips entirely when the buffer is empty, so file
/// count tracks the ARRIVAL PATTERN, not this timer. 60s therefore bought
/// almost nothing and cost up to 60s before a span became queryable.
///
/// Keep this low. The small-file problem is real but this is the wrong lever;
/// see roadmap #89 for the two that are not (accumulation-triggered
/// materialization of DuckLake inlined data, and `ducklake_merge_adjacent_files`
/// compaction). Do not raise it again without measuring file SIZE, not count.
pub(crate) static FLUSH_INTERVAL_SECS: Envar<u64> =
    Envar::on_startup("SIGIRO_FLUSH_INTERVAL_SECS", || EnvarDef::Default(1));

/// Row count threshold that triggers an immediate flush. Default: 10_000.
/// When this many rows are buffered across all signal tables, the flusher
/// writes them to DuckDB without waiting for the timer.
pub(crate) static FLUSH_ROW_THRESHOLD: Envar<usize> =
    Envar::on_startup("SIGIRO_FLUSH_ROW_THRESHOLD", || EnvarDef::Default(10_000));

/// How long `InsertRouter::shutdown_and_wait` gives each project's flusher to
/// drain before aborting it. Default: 5s.
///
/// The budget is wall-clock but the work is not: the flusher hands a Shutdown
/// command to a BLOCKING writer thread that drains into DuckDB, and that thread
/// competes for CPU and disk with everything else on the box. Five seconds is a
/// sound production budget — raising it globally would make a real shutdown hang
/// slower to detect — but a loaded machine can exceed it for reasons unrelated
/// to the code under test.
///
/// Operators draining a large buffer onto slow disk have the same problem and
/// the same lever.
pub(crate) static SHUTDOWN_TIMEOUT_SECS: Envar<u64> =
    Envar::on_startup("SIGIRO_SHUTDOWN_TIMEOUT_SECS", || EnvarDef::Default(5));

pub(crate) fn shutdown_timeout_secs() -> Result<u64> {
    let secs = SHUTDOWN_TIMEOUT_SECS.value()?;
    anyhow::ensure!(
        secs > 0,
        "SIGIRO_SHUTDOWN_TIMEOUT_SECS must be greater than zero"
    );
    Ok(secs)
}

/// Backpressure policy when the per-project channel is full. Default: "block".
/// - "block": sender awaits space (preserves data, may slow ingest)
/// - "drop-newest": drops the incoming message (prevents stalls, loses newest data)
pub(crate) static BACKPRESSURE_POLICY: Envar<String> =
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

pub(crate) fn backpressure_policy() -> Result<BackpressurePolicy> {
    match BACKPRESSURE_POLICY.value()?.as_str() {
        "block" => Ok(BackpressurePolicy::Block),
        "drop-newest" => Ok(BackpressurePolicy::DropNewest),
        other => anyhow::bail!(
            "unknown SIGIRO_BACKPRESSURE_POLICY: {other}. Expected 'block' or 'drop-newest'"
        ),
    }
}

pub(crate) fn duckdb_threads() -> Result<NonZeroUsize> {
    let raw = DUCKDB_THREADS.value()?;
    NonZeroUsize::new(raw)
        .ok_or_else(|| anyhow::anyhow!("SIGIRO_DUCKDB_THREADS must be greater than zero"))
}

pub(crate) fn duckdb_memory_limit() -> Result<String> {
    let memory_limit = DUCKDB_MEMORY_LIMIT.value()?;
    anyhow::ensure!(
        !memory_limit.trim().is_empty(),
        "SIGIRO_DUCKDB_MEMORY_LIMIT must not be empty"
    );
    Ok(memory_limit)
}

pub(crate) fn query_timeout_secs() -> Result<u64> {
    let secs = QUERY_TIMEOUT_SECONDS.value()?;
    anyhow::ensure!(
        secs > 0,
        "SIGIRO_QUERY_TIMEOUT_SECONDS must be greater than zero"
    );
    Ok(secs)
}

pub(crate) fn max_result_rows() -> Result<usize> {
    let rows = MAX_RESULT_ROWS.value()?;
    anyhow::ensure!(rows > 0, "SIGIRO_MAX_RESULT_ROWS must be greater than zero");
    Ok(rows)
}

/// Byte-size threshold that triggers an immediate flush. Default: 8_388_608 (8 MiB).
/// When the total buffered bytes across all signal tables reaches this limit,
/// the flusher writes to DuckDB without waiting for the row threshold or timer.
/// This prevents unbounded memory growth when individual rows are large.
pub(crate) static FLUSH_BYTE_THRESHOLD: Envar<usize> =
    Envar::on_startup("SIGIRO_FLUSH_BYTE_THRESHOLD", || {
        EnvarDef::Default(8_388_608)
    });

/// Max ingest request body size in bytes (OTLP HTTP `DefaultBodyLimit`, and
/// the matching gRPC `max_decoding_message_size` — see `crate::ingest`).
/// Default: 8_388_608 (8 MiB). The HTTP cap applies to the DECOMPRESSED size
/// (enforced when the body is collected into `Bytes`, which happens after
/// `RequestDecompressionLayer` has already un-gzipped it), so this also
/// bounds a gzip-bomb, not just the wire size.
static MAX_INGEST_BODY_BYTES: Envar<usize> =
    Envar::on_startup("SIGIRO_MAX_INGEST_BODY_BYTES", || {
        EnvarDef::Default(8_388_608)
    });

pub(crate) fn max_ingest_body_bytes() -> Result<NonZeroUsize> {
    NonZeroUsize::new(MAX_INGEST_BODY_BYTES.value()?)
        .context("SIGIRO_MAX_INGEST_BODY_BYTES must be greater than zero")
}

/// Hard cap on total bytes under `SIGIRO_DATA_DIR`. 0 = disabled (default).
/// When a periodic sample of the on-disk footprint reaches this, ingest is
/// rejected (HTTP 507 / gRPC `resource_exhausted`) until retention/cleanup
/// brings usage back under 90% of the cap (hysteresis). This is the backstop
/// that keeps a runaway box from filling its disk and crashing Postgres/the
/// host. In S3 mode it caps only the LOCAL catalog/inline/temp footprint, not
/// cold Parquet in the bucket. Raw bytes; `0` disables the gate.
///
/// This is a SAMPLED SOFT cap, not an instantaneous hard limit: the gate only
/// blocks NEW `send()` enqueues, so after a breach the already-queued backlog
/// still drains to disk. Post-breach overshoot is roughly one sampling window
/// of ingest (`sample_interval × ingest_rate`) plus the full queued backlog —
/// and that backlog scales with the number of active tenants, NOT a fixed
/// amount: each project has its own `PROJECT_CHANNEL_CAPACITY`-deep channel of
/// messages up to `SIGIRO_MAX_INGEST_BODY_BYTES` each, plus in-flush buffers.
/// SET THE CAP BELOW PHYSICAL DISK WITH ENOUGH HEADROOM TO ABSORB THAT. For the
/// single-tenant / few-project self-host-edge deployments this serves today the
/// backlog is small against a multi-GB margin (and DuckLake compresses on write,
/// so on-disk overshoot is well under the raw queued bytes); a many-tenant box
/// must size headroom to its project count. Do not set it at the disk's true
/// limit.
///
// ponytail: soft cap sized by headroom is right for single/few-tenant edges.
// A large multi-tenant deployment wants a per-tenant cap or a global
// buffered-bytes gate to bound the backlog directly — add with the hosted
// multi-tenant track, not before it exists.
pub(crate) static MAX_DISK_BYTES: Envar<u64> =
    Envar::on_startup("SIGIRO_MAX_DISK_BYTES", || EnvarDef::Default(0));

/// How often the storage-maintenance task samples the `SIGIRO_DATA_DIR`
/// on-disk byte size for the disk cap. Default: 30s.
static DISK_SAMPLE_SECONDS: Envar<u64> =
    Envar::on_startup("SIGIRO_DISK_SAMPLE_SECONDS", || EnvarDef::Default(30));

pub fn max_disk_bytes() -> Result<u64> {
    MAX_DISK_BYTES.value().map_err(anyhow::Error::from)
}

/// Consecutive DuckLake `CHECKPOINT` failures on one project's writer before
/// its ingest is shed with 507 / `resource_exhausted`.
///
/// A failed checkpoint materialises nothing: telemetry keeps accumulating in
/// the catalog while no Parquet is written. In distributed mode that catalog is
/// an external Postgres which `SIGIRO_MAX_DISK_BYTES` never samples, so the
/// disk gate cannot see the growth — sustained checkpoint failure is the only
/// signal available.
///
/// The default of 3 rides out a transient blip (one failed cycle recovers on
/// the next, and any success resets the count) while still bounding how long a
/// genuinely wedged storage backend can absorb unmaterialisable writes. Set 0
/// to disable shedding: failures are still counted and reported by `/health`,
/// but ingest keeps being accepted.
pub(crate) static MAX_CHECKPOINT_FAILURES: Envar<u32> =
    Envar::on_startup("SIGIRO_MAX_CHECKPOINT_FAILURES", || EnvarDef::Default(3));

pub(crate) fn max_checkpoint_failures() -> Result<u32> {
    MAX_CHECKPOINT_FAILURES.value().map_err(anyhow::Error::from)
}

pub(crate) fn disk_sample_seconds() -> Result<NonZeroU64> {
    NonZeroU64::new(DISK_SAMPLE_SECONDS.value()?)
        .context("SIGIRO_DISK_SAMPLE_SECONDS must be greater than zero")
}

/// Data directory for sigiro (holds DuckDB files, extensions, binaries).
/// Defaults to the platform data dir (~/.local/share/sigiro on Unix, %LOCALAPPDATA%\sigiro on Windows).
pub(crate) static DATA_DIR: Envar<String> =
    Envar::on_startup("SIGIRO_DATA_DIR", || EnvarDef::Unset);

/// Persistent hosted telemetry filesystem. Hosted mode never uses the local
/// platform data directory: PostgreSQL owns catalog state and this path owns
/// only local DuckLake data files, crash evidence, and migration evidence.
pub(crate) static HOSTED_DATA_DIR: Envar<String> =
    Envar::on_startup("SIGIRO_HOSTED_DATA_DIR", || {
        EnvarDef::Default("/var/lib/sigiro/telemetry".to_string())
    });

/// Shared writable DuckDB extension/cache directory for hosted mode.
pub(crate) static DUCKDB_CACHE_DIR: Envar<String> =
    Envar::on_startup("SIGIRO_DUCKDB_CACHE_DIR", || {
        EnvarDef::Default("/var/cache/sigiro/duckdb".to_string())
    });

pub fn hosted_data_dir() -> Result<camino::Utf8PathBuf> {
    Ok(camino::Utf8PathBuf::from(HOSTED_DATA_DIR.value()?))
}

pub(crate) fn duckdb_cache_dir() -> Result<camino::Utf8PathBuf> {
    Ok(camino::Utf8PathBuf::from(DUCKDB_CACHE_DIR.value()?))
}

/// Server URL for CLI client and admin commands. An empty value selects the
/// hosted endpoint after OAuth login and localhost otherwise.
pub static ENDPOINT: Envar<String> =
    Envar::on_startup("SIGIRO_ENDPOINT", || EnvarDef::Default(String::new()));

/// Public Blume documentation MCP endpoint. Override this for a self-hosted
/// docs deployment; no Sigiro credential is sent to this endpoint.
pub static DOCS_MCP_URL: Envar<String> = Envar::on_demand("SIGIRO_DOCS_MCP_URL", || {
    EnvarDef::Default("https://sigiro.com/mcp/docs".to_string())
});

/// Bearer token for a hosted JWT endpoint. Client-only: it is sent
/// as a bearer token by `client::try_build_client` and never configures
/// server-side authentication.
///
/// `on_demand`, not `on_startup`, because `main.rs` writes this variable from
/// the `--token` flag after startup and a cached read would miss it. Empty string
/// rather than `Unset` so the caller keeps its "no token means no auth" shape
/// without an error path.
pub(crate) static ACCESS_TOKEN: Envar<String> =
    Envar::on_demand("SIGIRO_ACCESS_TOKEN", || EnvarDef::Default(String::new()));

/// Better Auth OAuth issuer, protected-resource audience, and JWKS endpoint.
/// Set all three to make Sigiro a JWT resource server. Distributed storage
/// rejects startup unless all three are present; local/edge modes may be Open.
pub(crate) static AUTH_ISSUER: Envar<String> =
    Envar::on_demand("SIGIRO_AUTH_ISSUER", || EnvarDef::Default(String::new()));
pub(crate) static AUTH_AUDIENCE: Envar<String> =
    Envar::on_demand("SIGIRO_AUTH_AUDIENCE", || EnvarDef::Default(String::new()));
pub(crate) static AUTH_JWKS_URL: Envar<String> =
    Envar::on_demand("SIGIRO_AUTH_JWKS_URL", || EnvarDef::Default(String::new()));

pub(crate) fn jwt_resource_server_config() -> Result<Option<(String, String, String)>> {
    let issuer = AUTH_ISSUER.value()?;
    let audience = AUTH_AUDIENCE.value()?;
    let jwks_url = AUTH_JWKS_URL.value()?;
    if issuer.is_empty() && audience.is_empty() && jwks_url.is_empty() {
        return Ok(None);
    }
    anyhow::ensure!(
        !issuer.is_empty() && !audience.is_empty() && !jwks_url.is_empty(),
        "SIGIRO_AUTH_ISSUER, SIGIRO_AUTH_AUDIENCE, and SIGIRO_AUTH_JWKS_URL must be set together"
    );
    Ok(Some((issuer, audience, jwks_url)))
}

/// Path to the direct-ingest TLS certificate PEM file (ports 4317 and 4318).
/// The main HTTP API stays plaintext for a TLS-terminating reverse proxy.
pub(crate) static TLS_CERT: Envar<String> =
    Envar::on_startup("SIGIRO_TLS_CERT", || EnvarDef::Unset);
/// Path to the direct-ingest TLS private key PEM file. Unset = no TLS.
pub(crate) static TLS_KEY: Envar<String> = Envar::on_startup("SIGIRO_TLS_KEY", || EnvarDef::Unset);

/// Data retention days. 0 = disabled (no automatic cleanup). Default 7 —
/// retention is ON by default so an unattended box reclaims disk instead of
/// filling up. Each per-table DELETE is followed by a DuckLake snapshot-expiry
/// and file-cleanup pass (`inserter::reclaim_ducklake_disk`), because a DELETE
/// writes only delete-files and frees no bytes on its own.
pub(crate) static RETENTION_DAYS: Envar<u32> =
    Envar::on_startup("SIGIRO_RETENTION_DAYS", || EnvarDef::Default(7));

/// PostgreSQL catalog URL (required for distributed/multi-tenant mode, optional
/// otherwise). Unset means the local DuckDB file catalog (`catalog.ducklake`).
/// Deployment modes: docs/architecture/deployment-modes.html
pub(crate) static CATALOG_URL: Envar<String> =
    Envar::on_startup("SIGIRO_CATALOG_URL", || EnvarDef::Unset);

/// Which cross-project query surface (if any) this node exposes.
/// - "none" (default): no cross-project query surface.
/// - "quack": the DuckDB `quack` extension serves this node's DuckLake data
///   for external ATTACH. Inert until the query-surface feature lands.
pub(crate) static QUERY_SURFACE: Envar<String> = Envar::on_startup("SIGIRO_QUERY_SURFACE", || {
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
pub(crate) static QUACK_BIND: Envar<String> = Envar::on_startup("SIGIRO_QUACK_BIND", || {
    EnvarDef::Default("127.0.0.1".to_string())
});

/// Port for the quack query-serving surface.
pub(crate) static QUACK_PORT: Envar<u16> =
    Envar::on_startup("SIGIRO_QUACK_PORT", || EnvarDef::Default(7777));

/// Optional bearer token quack clients must present. Empty (the default)
/// means no token — an acceptable choice only because the bind address
/// above defaults to loopback and any wider exposure is the operator's own,
/// explicit choice.
static QUACK_TOKEN: Envar<String> =
    Envar::on_demand("SIGIRO_QUACK_TOKEN", || EnvarDef::Default(String::new()));

pub(crate) fn quack_token() -> Result<Option<crate::quack::QuackToken>> {
    let raw = QUACK_TOKEN.value()?;
    if raw.is_empty() {
        return Ok(None);
    }
    crate::quack::QuackToken::try_new(raw)
        .map(Some)
        .map_err(|error| anyhow::anyhow!("invalid SIGIRO_QUACK_TOKEN: {error}"))
}

/// Federated query coordinator (`sigiro query`) edge override: comma-separated
/// `name=host:port` entries. When set (together with any `--edge` CLI flags),
/// used verbatim and tailnet discovery (peers tagged `tag:sigiro-edge`) is
/// skipped entirely. Empty (the default) means: discover from the tailnet.
pub(crate) static EDGES: Envar<String> =
    Envar::on_demand("SIGIRO_EDGES", || EnvarDef::Default(String::new()));

/// Row-count safety cap enforced on every per-edge partial query result in
/// the federated query coordinator. Protects against a query with no
/// prunable time bound (or a non-selective one) pulling unbounded rows
/// across the network from a single edge. Default: 1_000_000.
pub(crate) static QUERY_ROW_CAP: Envar<usize> =
    Envar::on_startup("SIGIRO_QUERY_ROW_CAP", || EnvarDef::Default(1_000_000));

pub(crate) fn query_row_cap() -> Result<usize> {
    let cap = QUERY_ROW_CAP.value()?;
    anyhow::ensure!(cap > 0, "SIGIRO_QUERY_ROW_CAP must be greater than zero");
    Ok(cap)
}

/// Admin bearer token for distributed tenant/key provisioning endpoints.
/// Unset disables those mutation endpoints.
pub(crate) static ADMIN_TOKEN: Envar<String> =
    Envar::on_startup("SIGIRO_ADMIN_TOKEN", || EnvarDef::Unset);
