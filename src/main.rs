use anyhow::{Context, Result};
use clap::{Parser, Subcommand, builder::styling};
use parking_lot::Mutex;
use std::io::IsTerminal;
use std::sync::Arc; // for stdout().is_terminal()

use sigiro::responses::HealthResponse;
use sigiro::{data_dir, inserter::MsgRouter, pool::ProjectPool, profiling::start_profiling};
use std::sync::atomic::AtomicBool;

const STYLES: styling::Styles = styling::Styles::styled()
    .header(styling::AnsiColor::Cyan.on_default().bold())
    .usage(styling::AnsiColor::Cyan.on_default().bold())
    .literal(styling::AnsiColor::Green.on_default().bold())
    .placeholder(styling::AnsiColor::Yellow.on_default());

#[derive(Parser)]
#[command(name = "sigiro", version, about = "Observability for AI agents", styles = STYLES)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Output format (toon or json). When unset, auto-detects: JSON if stdout
    /// is not a TTY (piped to jq, a file, etc.), TOON otherwise.
    #[arg(long, value_enum)]
    output: Option<OutputFormat>,
    /// Client bearer token for a hosted endpoint (overrides SIGIRO_API_KEY).
    #[arg(long, global = true)]
    key: Option<String>,
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum OutputFormat {
    /// Token-Oriented Object Notation (default, ~40% fewer tokens)
    Toon,
    /// Standard JSON format
    Json,
}

#[derive(Subcommand)]
enum Command {
    /// Start gRPC + HTTP servers
    Serve {
        /// Data directory (overrides SIGIRO_DATA_DIR)
        #[arg(long)]
        data_dir: Option<String>,
        /// HTTP API port (overrides SIGIRO_HTTP_PORT)
        #[arg(long)]
        http_port: Option<u16>,
        /// OTLP HTTP port (overrides SIGIRO_OTLP_HTTP_PORT)
        #[arg(long)]
        otlp_http_port: Option<u16>,
        /// OTLP gRPC port (overrides SIGIRO_GRPC_PORT)
        #[arg(long)]
        otlp_grpc_port: Option<u16>,
    },
    /// Run a federated SQL query across one or more sigiro edges.
    ///
    /// This is a coordinator: an embedded DuckDB plans the query, pushes a
    /// per-edge partial to each resolved edge over the `quack` wire protocol
    /// (SIGIRO_QUERY_SURFACE=quack on the edge), and folds the results
    /// (never avg-of-avgs — see `sigiro::federate`). Edges are resolved from
    /// the tailnet (peers tagged tag:sigiro-edge) unless overridden.
    Query {
        /// SQL statement to execute
        sql: Option<String>,
        /// Path to a .sql file to execute (use - for stdin)
        #[arg(long, value_name = "PATH")]
        sql_file: Option<String>,
        /// Edge override: name=host:port. Repeatable. Also settable via
        /// SIGIRO_EDGES (comma-separated). When either is given, tailnet
        /// discovery is skipped entirely.
        #[arg(long = "edge", value_name = "NAME=HOST:PORT")]
        edge: Vec<String>,
        /// Allow a query with no WHERE bound on the timestamp column — the
        /// un-prunable cross-edge blowup case. Refused by default.
        #[arg(long)]
        full_scan: bool,
    },
    /// Show server health
    Status,
    /// Investigate a service — returns structured metrics, spans, logs, linked traces
    Investigate {
        /// Service name to investigate
        service: String,
        /// Start of range in epoch seconds. Defaults to 15 minutes ago if omitted.
        #[arg(long)]
        from: Option<i64>,
        /// End of range in epoch seconds. Defaults to now if omitted.
        #[arg(long)]
        to: Option<i64>,
        /// F-23: client-side filter for span/operation names (case-insensitive
        /// substring). Useful for narrowing a busy investigate output.
        #[arg(long)]
        grep: Option<String>,
    },
    /// Diagnose sigiro using its own telemetry — no docker logs
    Diagnose,
    /// Pre-download DuckDB extensions into the data dir so the first
    /// `sigiro serve` doesn't hang on a 30-90s blocking download (F-01).
    /// Use this once after installing sigiro, or before an offline deploy.
    FetchExtensions,
    /// Probe the local /health endpoint and exit 0 if ready, non-zero
    /// otherwise. This is the container HEALTHCHECK for the distroless image,
    /// which has no shell or curl for the usual `curl -f /health` probe.
    Healthcheck,
}

/// Format output. If the user did not explicitly pass --output, and stdout
/// is not a TTY (we are being piped to jq, a file, another process, etc.),
/// default to JSON. TOON is great for humans at a terminal but unfriendly
/// to jq / grep / any text pipeline.
fn format_output(
    value: &serde_json::Value,
    output_format: Option<&OutputFormat>,
) -> Result<String> {
    let use_json = output_format.is_none() && !std::io::stdout().is_terminal();
    let fmt = output_format.unwrap_or(&OutputFormat::Toon);
    match (use_json, fmt) {
        (true, _) | (false, OutputFormat::Json) => Ok(serde_json::to_string_pretty(value)?),
        (false, OutputFormat::Toon) => Ok(toon::encode(value, None)),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install a panic hook that writes a breadcrumb to stderr and to
    // data_dir/crash.log so any future silent death leaves a trace
    // (F-02, F-29). The default panic hook only writes to stderr and
    // can be missed when stdout is captured by nohup / docker.
    std::panic::set_hook(Box::new(|info| {
        let bt = std::backtrace::Backtrace::force_capture();
        eprintln!("\n[panic] {info}\n[backtrace]\n{bt}");
        let ts = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "unknown-time".to_string());
        let crash_dir = std::env::var("SIGIRO_DATA_DIR")
            .ok()
            .map(camino::Utf8PathBuf::from)
            .or_else(|| sigiro::data_dir().ok());
        if let Some(dir) = crash_dir {
            drop(std::fs::create_dir_all(&dir));
            drop(std::fs::write(
                dir.join("crash.log"),
                format!("[{ts}] [panic] {info}\n[backtrace]\n{bt}\n"),
            ));
        }
    }));

    // OTel self-instrumentation: read config at startup, pass to init_telemetry()
    let telemetry_config = sigiro::otel::TelemetryConfig::from_env()?;
    let _telemetry_guard = match sigiro::otel::init_telemetry(telemetry_config) {
        Ok(Some(guard)) => Some(guard),
        Ok(None) => {
            init_basic_subscriber();
            None
        }
        Err(e) => {
            init_basic_subscriber();
            tracing::warn!("OTel self-instrumentation failed to initialize: {e:#}");
            None
        }
    };

    let cli = Cli::parse();

    // This is client-only configuration. Apply it before any command resolves
    // endpoint configuration, so `--key` has the same behavior as
    // SIGIRO_API_KEY without changing server authentication.
    if let Some(key) = &cli.key {
        unsafe { std::env::set_var("SIGIRO_API_KEY", key) };
    }

    match cli.command {
        Some(Command::Serve {
            data_dir,
            http_port,
            otlp_http_port,
            otlp_grpc_port,
        }) => {
            // Apply CLI flags to env before any config::*.value() is called.
            // The on_startup Envar machinery reads env::var at first .value(),
            // so this is the only correct ordering. std::env::set_var is
            // marked unsafe in edition 2024 because it's not thread-safe; we
            // are single-threaded here (before tokio starts the runtime).
            unsafe {
                if let Some(v) = data_dir {
                    std::env::set_var("SIGIRO_DATA_DIR", v);
                }
                if let Some(v) = http_port {
                    std::env::set_var("SIGIRO_HTTP_PORT", v.to_string());
                }
                if let Some(v) = otlp_http_port {
                    std::env::set_var("SIGIRO_OTLP_HTTP_PORT", v.to_string());
                }
                if let Some(v) = otlp_grpc_port {
                    std::env::set_var("SIGIRO_GRPC_PORT", v.to_string());
                }
            }
            run_serve().await?;
        }
        None => {
            run_serve().await?;
        }
        Some(Command::Query {
            sql,
            sql_file,
            edge,
            full_scan,
        }) => {
            // F-14: support --sql-file for long queries, reuse file across runs,
            // or piping `cat foo.sql | sigiro query --sql-file -`
            let sql = match (sql, sql_file) {
                (Some(s), _) => s,
                (None, Some(path)) if path == "-" => {
                    use std::io::Read;
                    let mut s = String::new();
                    std::io::stdin().read_to_string(&mut s)?;
                    s
                }
                (None, Some(path)) => std::fs::read_to_string(&path)
                    .with_context(|| format!("failed to read SQL file {path}"))?,
                (None, None) => anyhow::bail!("provide SQL as argument or via --sql-file"),
            };
            let result = sigiro::federate::run_federated_query(&sql, &edge, full_scan).await?;
            let value = serde_json::to_value(&result)?;
            println!("{}", format_output(&value, cli.output.as_ref())?);
        }
        Some(Command::Status) => {
            // Health is public — no auth needed. The /health endpoint is the
            // single source of truth for "ready" vs "starting" vs "degraded";
            // the CLI does not read the .ready file directly (would be a
            // second path that can drift from the server's view).
            let base_url = sigiro::config::ENDPOINT
                .value()
                .unwrap_or_else(|_| "http://localhost:9999".to_string());
            let health_client =
                sigiro::client::SigiroClient::new(&base_url, sigiro::client::ClientAuth::Open);
            let (state, detail) = match health_client.health().await {
                Ok(HealthResponse::Ready) => ("ready".to_string(), "ok".to_string()),
                Ok(HealthResponse::Starting) => (
                    "starting".to_string(),
                    "http up, init not complete".to_string(),
                ),
                Ok(HealthResponse::Degraded { reason }) => ("degraded".to_string(), reason),
                Err(e) => ("unreachable".to_string(), format!("{e:#}")),
            };
            // F-21/F-31: include a recent_ingest block so the user can see at a
            // glance whether the server is actually receiving data. If auth or
            // query setup fails, report the exact failure instead of silently
            // collapsing it to null.
            let recent_ingest = if matches!(state.as_str(), "ready" | "starting") {
                match sigiro::client::try_build_client() {
                    Ok(client) => match client.query(
                        "SELECT (SELECT count(*) FROM lake.sigiro_spans WHERE timestamp > CAST(CURRENT_TIMESTAMP AS TIMESTAMP) - interval 5 minute) AS spans_5m, (SELECT count(*) FROM lake.sigiro_logs WHERE timestamp > CAST(CURRENT_TIMESTAMP AS TIMESTAMP) - interval 5 minute) AS logs_5m, CAST((SELECT max(timestamp) FROM lake.sigiro_spans) AS VARCHAR) AS last_span_ts, CAST((SELECT max(timestamp) FROM lake.sigiro_logs) AS VARCHAR) AS last_log_ts",
                    ).await {
                        Ok(rows) => rows.into_iter().next().map(|r| {
                            serde_json::json!({
                                "spans_5m": r.get("spans_5m").cloned().unwrap_or(serde_json::Value::Null),
                                "logs_5m": r.get("logs_5m").cloned().unwrap_or(serde_json::Value::Null),
                                "last_span_ts": r.get("last_span_ts").cloned().unwrap_or(serde_json::Value::Null),
                                "last_log_ts": r.get("last_log_ts").cloned().unwrap_or(serde_json::Value::Null),
                            })
                        }).or_else(|| Some(serde_json::json!({
                            "error": "empty_query_result",
                            "detail": "recent ingest query returned no rows",
                        }))),
                        Err(e) => Some(serde_json::json!({
                            "error": "query_error",
                            "detail": format!("{e:#}"),
                        })),
                    },
                    Err(e) => Some(serde_json::json!({
                        "error": "auth_config_error",
                        "detail": format!("{e:#}"),
                    })),
                }
            } else {
                None
            };
            let value = serde_json::json!({
                "status": state,
                "detail": detail,
                "recent_ingest": recent_ingest,
            });
            println!("{}", format_output(&value, cli.output.as_ref())?);
        }
        Some(Command::Investigate {
            service,
            from,
            to,
            grep,
        }) => {
            let client = require_client()?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
            let from_ts = from.map(|s| s * 1_000_000).unwrap_or_else(|| {
                tracing::info!("--from not specified, defaulting to 15 minutes ago");
                (now - 900) * 1_000_000
            });
            let to_ts = to.map(|s| s * 1_000_000).unwrap_or(now * 1_000_000);
            let mut block = client.investigate(&service, from_ts, to_ts).await?;
            // F-23: client-side --grep filter. Filter operation summaries,
            // log patterns, and spans by case-insensitive substring.
            if let Some(pattern) = grep {
                let pat = pattern.to_lowercase();
                block
                    .operation_summaries
                    .retain(|op| op.span_name.to_lowercase().contains(&pat));
                block
                    .log_patterns
                    .retain(|lp| lp.pattern.to_lowercase().contains(&pat));
                block.spans.sample.retain(|s| {
                    s.trace_id.to_lowercase().contains(&pat)
                        || s.span_name.to_lowercase().contains(&pat)
                });
            }
            // No ad-hoc trace hint anymore: every span carries its own
            // `drill_down_sql` (the full trace) and each sampled section its
            // `drill_all_sql`, so the response is self-describing.
            let value = serde_json::to_value(&block)?;
            println!("{}", format_output(&value, cli.output.as_ref())?);
        }
        Some(Command::Diagnose) => {
            let client = require_client()?;
            sigiro::diagnose::run(&client).await?;
        }
        Some(Command::FetchExtensions) => {
            let dir = data_dir()?;
            std::fs::create_dir_all(&dir)?;
            sigiro::storage::prewarm_extensions(&dir)?;
            println!(
                "DuckDB extensions pre-warmed into {}",
                sigiro::storage::resolve_extension_dir(&dir)
            );
        }
        Some(Command::Healthcheck) => {
            // Shell-free liveness probe for the distroless container HEALTHCHECK.
            // /health is public (no auth). Exit 0 only when fully Ready so the
            // orchestrator doesn't route to a still-initializing instance.
            let base_url = sigiro::config::ENDPOINT
                .value()
                .unwrap_or_else(|_| "http://localhost:9999".to_string());
            let client =
                sigiro::client::SigiroClient::new(&base_url, sigiro::client::ClientAuth::Open);
            match client.health().await {
                Ok(HealthResponse::Ready) => {}
                other => {
                    eprintln!("healthcheck: not ready: {other:?}");
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}

async fn run_serve() -> Result<()> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir)?;
    tracing::info!(
        "sigiro starting — data_dir={}, first-run extension download may take 30-90s",
        dir
    );

    // Init-complete flag — flipped at the tail of init so /health reports
    // Ready only after the server is actually serving traffic. Owned by
    // run_serve, borrowed by AppState via Arc<AtomicBool>.
    let init_complete = Arc::new(AtomicBool::new(false));

    // Resolve the storage mode ONCE, up front, validating the config as a
    // coherent axis (fail-loud on incoherence, e.g. S3 bucket without a catalog
    // URL). Threaded into the pool so every per-project attach uses this single
    // decision instead of re-reading the environment.
    let storage_mode = sigiro::storage::StorageMode::from_env()?;

    // MsgRouter: handlers send directly to per-project flusher tasks.
    let pool = Arc::new(ProjectPool::new(dir.clone(), 100, storage_mode));
    let admin_conn = sigiro::admin::open_admin_db()?;

    // Write instance status at startup
    sigiro::admin::write_instance_status(&admin_conn, "version", env!("CARGO_PKG_VERSION"))?;
    sigiro::admin::write_instance_status(&admin_conn, "mode", "otlp")?;
    sigiro::admin::write_instance_status(&admin_conn, "platform_os", std::env::consts::OS)?;
    sigiro::admin::write_instance_status(&admin_conn, "platform_arch", std::env::consts::ARCH)?;
    let kernel = unsafe {
        let mut utsname: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut utsname) == 0 {
            std::ffi::CStr::from_ptr(utsname.release.as_ptr())
                .to_string_lossy()
                .into_owned()
        } else {
            "unknown".to_string()
        }
    };
    sigiro::admin::write_instance_status(&admin_conn, "platform_kernel", &kernel)?;
    #[cfg(target_os = "linux")]
    {
        let has_bpf = caps::has_cap(None, caps::CapSet::Effective, caps::Capability::CAP_BPF)
            .unwrap_or(false);
        let has_net_admin = caps::has_cap(
            None,
            caps::CapSet::Effective,
            caps::Capability::CAP_NET_ADMIN,
        )
        .unwrap_or(false);
        sigiro::admin::write_instance_status(
            &admin_conn,
            "capability_bpf",
            if has_bpf { "true" } else { "false" },
        )?;
        sigiro::admin::write_instance_status(
            &admin_conn,
            "capability_net_admin",
            if has_net_admin { "true" } else { "false" },
        )?;
    }
    #[cfg(not(target_os = "linux"))]
    {
        sigiro::admin::write_instance_status(&admin_conn, "capability_bpf", "false")?;
        sigiro::admin::write_instance_status(&admin_conn, "capability_net_admin", "false")?;
    }

    // Extension probe is deferred to first project connection (pool.create_project).
    // Use `sigiro fetch-extensions` to pre-download for offline deployments.

    // Pre-create the "default" project.
    //
    // - Quack enabled (SIGIRO_QUERY_SURFACE=quack): ALWAYS pre-create,
    //   regardless of storage mode. The quack query surface is served over a
    //   connection *cloned from the "default" project's* pool entry (see
    //   crate::server::run_servers's `pool.clone_connection("default")`) —
    //   this holds whether "default" is a local DuckDB-file project or a
    //   distributed (SIGIRO_CATALOG_URL) project backed by S3 cold storage
    //   (the single-edge "Option A" quack-over-S3 topology: one distributed
    //   project literally named "default", reusing the same distributed
    //   ATTACH path every other tenant uses — see src/storage.rs
    //   `sanitize_tenant_id`, which no longer reserves "default"). A
    //   pre-create failure here used to be downgraded to a non-fatal WARN
    //   unconditionally, which let boot continue into a server that then
    //   failed minutes later with the misleading "connection not found for
    //   project default — call create_project() first" instead of this, its
    //   real cause. Fail loud here instead.
    // - Quack disabled + local storage: pre-create so ad-hoc local queries
    //   work without ingest first; non-fatal on failure (existing behavior).
    // - Quack disabled + distributed storage: skip — tenants (including one
    //   that might happen to be named "default") are created explicitly
    //   through the control plane; there is no single edge here that needs
    //   "default" to exist up front.
    let storage_mode = pool.storage_mode();
    let distributed = storage_mode.is_distributed();
    let quack_enabled = sigiro::config::query_surface()? == sigiro::config::QuerySurface::Quack;

    if quack_enabled {
        pool.create_project("default").map_err(|e| {
            let mut chain = format!("{e}");
            for cause in e.chain().skip(1) {
                chain.push_str(&format!("\n  Caused by: {cause}"));
            }
            anyhow::anyhow!(
                "default project init failed and SIGIRO_QUERY_SURFACE=quack requires it (the \
                 quack query surface serves from the \"default\" project's connection) — \
                 refusing to boot into a server that would fail later with a misleading \
                 \"connection not found\" instead:\n{chain}"
            )
        })?;
        let topology = if storage_mode.uses_s3() {
            "distributed S3 project default"
        } else if storage_mode.is_distributed() {
            "distributed local-data project default"
        } else {
            "local project default"
        };
        tracing::info!("serving quack edge — {topology}");
    } else if distributed {
        tracing::info!("distributed storage configured — skipping default project pre-create");
    } else {
        match pool.create_project("default") {
            Ok(_) => tracing::info!("default project initialized"),
            Err(e) => {
                let mut chain = format!("{e}");
                for cause in e.chain().skip(1) {
                    chain.push_str(&format!("\n  Caused by: {cause}"));
                }
                tracing::warn!("default project init failed (non-fatal): {chain}");
            }
        }
    }

    // eBPF capability probe: read-only "could this host run eBPF
    // auto-instrumentation" check, recorded to instance_status. The launcher
    // itself is a future feature — there is nothing to start here yet.
    let ebpf_status = sigiro::diagnose::ebpf_status("obi");
    sigiro::diagnose::write_ebpf_status(&admin_conn, &ebpf_status)?;
    if ebpf_status.status != "ready" {
        tracing::info!(
            "eBPF unavailable: {} — running in OTLP-only mode",
            ebpf_status.reason
        );
    }

    // Pyroscope continuous profiling: enabled when SIGIRO_PYROSCOPE_ENDPOINT is set.
    // Sends CPU + memory allocation profiles to the Pyroscope server.
    // The smoke-test script fetches these profiles and includes them in investigate blocks.
    let _profiling = match start_profiling() {
        Ok(Some(handle)) => {
            sigiro::admin::write_instance_status(&admin_conn, "pyroscope_status", "active")?;
            Some(handle)
        }
        Ok(None) => {
            sigiro::admin::write_instance_status(&admin_conn, "pyroscope_status", "disabled")?;
            None
        }
        Err(e) => {
            tracing::warn!("Pyroscope profiling failed to start: {e:#}");
            sigiro::admin::write_instance_status(&admin_conn, "pyroscope_status", "error")?;
            None
        }
    };

    tracing::info!("sigiro ready — data_dir={}", dir);
    // F-03: flip the typed init-complete flag so /health reports Ready.
    // The previous .ready file approach required a disk write on every
    // server start (silently dropped on failure) plus a disk read on every
    // health probe. In-memory state is faster, type-checked, and cannot
    // be forgotten by `run_serve` (we're already here at the tail).
    init_complete.store(true, std::sync::atomic::Ordering::Release);

    let shared_admin = Arc::new(Mutex::new(admin_conn));

    let disk_cap = sigiro::config::max_disk_bytes()?;
    let disk_state = sigiro::inserter::DiskCapacityState::new(disk_cap);
    let router = Arc::new(MsgRouter::new_with_disk(Arc::clone(&pool), disk_state));
    router.start();

    // Evict idle connections every 2 minutes
    const EVICT_INTERVAL_SECS: u64 = 120;
    const MAX_IDLE_SECS: u64 = 300;
    tokio::spawn(run_evictor(
        Arc::clone(&pool),
        EVICT_INTERVAL_SECS,
        MAX_IDLE_SECS,
    ));

    sigiro::server::run_servers(pool, shared_admin, router, dir.clone(), init_complete).await?;
    Ok(())
}

#[tracing::instrument(skip_all)]
async fn run_evictor(pool: Arc<ProjectPool>, interval_secs: u64, max_idle_secs: u64) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
    loop {
        interval.tick().await;
        pool.evict_idle(std::time::Duration::from_secs(max_idle_secs));
    }
}

/// Build HTTP client from config. Fails fast if API key or endpoint is not configured.
fn require_client() -> Result<sigiro::client::SigiroClient> {
    sigiro::client::try_build_client()
}

fn init_basic_subscriber() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sigiro=info".parse().expect("hardcoded filter is valid")),
        )
        .with_writer(std::io::stderr)
        .init();
}
