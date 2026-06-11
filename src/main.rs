use anyhow::{Context, Result};
use clap::{Parser, Subcommand, builder::styling};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;
use std::sync::Arc; // for stdout().is_terminal()

use itertools::Itertools;
use sigiro::responses::{HealthResponse, SpanDetail};
use sigiro::toon::to_toon;
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
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum OutputFormat {
    /// Token-Oriented Object Notation (default, ~40% fewer tokens)
    Toon,
    /// Standard JSON format
    Json,
}

#[derive(clap::ValueEnum, Clone, Debug)]
pub enum TopologyFormat {
    /// ASCII tree format (default)
    Ascii,
    /// Mermaid diagram format
    Mermaid,
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
    /// Run SQL query
    Query {
        /// SQL statement to execute
        sql: Option<String>,
        /// Path to a .sql file to execute (use - for stdin)
        #[arg(long, value_name = "PATH")]
        sql_file: Option<String>,
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
    /// Show trace waterfall — parent/child span tree with timing
    Waterfall {
        /// Trace ID (hex string) to display
        trace_id: String,
    },
    /// Show ASCII dependency graph
    Topology {
        /// Service name to focus on (optional - shows full graph if omitted)
        service: Option<String>,
        /// Output format (ascii or mermaid)
        #[arg(long, default_value = "ascii")]
        format: TopologyFormat,
    },
    /// Show hierarchical trace tree
    TraceTree {
        /// Trace ID (hex string) to display
        trace_id: String,
    },
    /// Save API key for CLI authentication
    Auth {
        /// API key (set SIGIRO_API_KEY on the server to the same value)
        key: String,
    },
    /// Diagnose sigiro using its own telemetry — no docker logs
    Diagnose,
    /// Pre-download DuckDB extensions into the data dir so the first
    /// `sigiro serve` doesn't hang on a 30-90s blocking download (F-01).
    /// Use this once after installing sigiro, or before an offline deploy.
    FetchExtensions,
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
        (false, OutputFormat::Toon) => Ok(to_toon(value)),
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
        Some(Command::Query { sql, sql_file }) => {
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
            let client = require_client()?;
            let result = client.query(&sql).await?;
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
            // F-21: include a recent_ingest block so the user can see at a
            // glance whether the server is actually receiving data. Tolerate
            // failure (e.g. when not authenticated) and return null.
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
                        }),
                        Err(_) => None,
                    },
                    Err(_) => None,
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
            // log patterns, and traces by case-insensitive substring.
            if let Some(pattern) = grep {
                let pat = pattern.to_lowercase();
                block
                    .operation_summaries
                    .retain(|op| op.span_name.to_lowercase().contains(&pat));
                block
                    .log_patterns
                    .retain(|lp| lp.pattern.to_lowercase().contains(&pat));
                block.traces.retain(|t| {
                    t.trace_id.to_lowercase().contains(&pat)
                        || t.root_span.span_name.to_lowercase().contains(&pat)
                });
                block.linked_traces.retain(|t| {
                    t.trace_id.to_lowercase().contains(&pat)
                        || t.root_span.span_name.to_lowercase().contains(&pat)
                });
            }
            let mut value = serde_json::to_value(&block)?;
            // F-12: include a hint in the response telling the user how to
            // follow a trace_id they find. The agent then can run
            // `sigiro waterfall <trace_id>` or `curl /v1/traces/<id>`.
            if let serde_json::Value::Object(ref mut map) = value {
                let base = sigiro::config::ENDPOINT
                    .value()
                    .unwrap_or_else(|_| "http://localhost:9999".to_string());
                map.insert(
                    "_follow_trace_hint".to_string(),
                    serde_json::json!(format!(
                        "for any trace_id above, run: sigiro waterfall <id>  or  curl {}/v1/traces/<id>",
                        base
                    )),
                );
            }
            println!("{}", format_output(&value, cli.output.as_ref())?);
        }
        Some(Command::Waterfall { trace_id }) => {
            run_waterfall(&trace_id).await?;
        }
        Some(Command::Topology { service, format }) => {
            run_topology(service.as_deref(), format).await?;
        }
        Some(Command::TraceTree { trace_id }) => {
            run_trace_tree(&trace_id).await?;
        }
        Some(Command::Auth { key }) => {
            let path = sigiro::config::save_api_key(&key)?;
            println!("API key saved to {}", path);
        }
        Some(Command::Diagnose) => {
            let client = require_client()?;
            sigiro::diagnose::run(&client).await?;
        }
        Some(Command::FetchExtensions) => {
            let dir = data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let project_dir = dir.join("default");
            std::fs::create_dir_all(&project_dir)?;
            sigiro::storage::prewarm_extensions(&project_dir)?;
            println!("DuckDB extensions pre-warmed into {}", project_dir);
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

    // MsgRouter: handlers send directly to per-project flusher tasks.
    let pool = Arc::new(ProjectPool::new(dir.clone(), 100));
    let admin_conn = sigiro::admin::open_admin_db()?;

    sigiro::federation::create_workers_table(&admin_conn)
        .context("failed to create federation workers table")?;

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

    // Pre-create default project so queries work without ingest
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

    // eBPF auto-instrumentation: try obi, degrade gracefully
    let ebpf_enabled = sigiro::config::EBPF_ENABLED.value().unwrap_or(false);
    if !ebpf_enabled {
        tracing::info!("eBPF disabled (set SIGIRO_EBPF=true to enable)");
        sigiro::admin::write_instance_status(&admin_conn, "ebpf_status", "disabled")?;
        sigiro::admin::write_instance_status(&admin_conn, "ebpf_reason", "SIGIRO_EBPF not set")?;
    } else {
        let otlp_endpoint = sigiro::config::OTEL_EXPORTER_OTLP_ENDPOINT
            .value()
            .map_err(anyhow::Error::from)?;
        let status = sigiro::ebpf::ebpf_status("obi");
        sigiro::ebpf::write_ebpf_status(&admin_conn, &status)?;
        match status.status.as_str() {
            "ready" => {
                sigiro::ebpf::start_obi("obi", &otlp_endpoint, &())?;
            }
            _ => {
                tracing::warn!(
                    "eBPF unavailable: {} — running in OTLP-only mode",
                    status.reason
                );
            }
        }
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

    // Federation central node requires a shared catalog (PostgreSQL).
    // Local SQLite cannot be shared across nodes.
    let central = sigiro::config::CENTRAL.value().unwrap_or(false);
    if central {
        match sigiro::config::CATALOG_URL.value() {
            Ok(ref url) if !url.is_empty() => {}
            _ => anyhow::bail!(
                "Federation central node requires SIGIRO_CATALOG_URL (PostgreSQL catalog). \
                 Local SQLite cannot be shared across nodes. \
                 Set SIGIRO_CATALOG_URL=postgresql://user:pass@host/db to enable federation."
            ),
        }
    }

    // Federation: start quack server on any node that has quack config.
    // Central nodes use it to serve federated catalog metadata;
    // collectors use it to expose their local DuckLake data for remote ATTACH.
    // The server runs in a dedicated background thread because `quack_serve` is a
    // streaming table function — dropping the `Rows` iterator cancels the query
    // and shuts the server down.
    if let (Ok(quack_port), Ok(quack_token)) = (
        sigiro::config::QUACK_PORT.value(),
        sigiro::config::QUACK_TOKEN.value(),
    ) {
        if central {
            tracing::info!("running as central federation node");
        }
        let quack_conn = pool.open_federation_connection()?;
        let token = quack_token.clone();
        std::thread::spawn(move || {
            if let Err(e) = sigiro::quack_federation::start_quack_server_blocking(
                quack_conn, quack_port, &token,
            ) {
                tracing::error!("quack server thread exited: {e:#}");
            }
        });
        // Brief pause so the thread has time to bind the port before we continue.
        std::thread::sleep(std::time::Duration::from_millis(300));
        tracing::info!("quack server started on port {}", quack_port);
    }

    // Federation: collector self-registration with central node
    if let Ok(ref url) = sigiro::config::CENTRAL_URL.value() {
        tracing::info!("collector self-registering with central node at {url}");
        let api_key = sigiro::config::API_KEY.value().unwrap_or_default();
        let auth = sigiro::client::ClientAuth::from_key(api_key);
        let client = sigiro::client::SigiroClient::new(url, auth);
        let status = sigiro::admin::read_all_status(&admin_conn)?;
        let quack_port = sigiro::config::QUACK_ADVERTISE_PORT
            .value()
            .ok()
            .or_else(|| sigiro::config::QUACK_PORT.value().ok())
            .unwrap_or(5432);
        let endpoint = sigiro::config::ADVERTISE_ENDPOINT
            .value()
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| sigiro::config::ENDPOINT.value().ok())
            .unwrap_or_else(|| "http://localhost:9999".to_string());
        let worker = sigiro::federation::WorkerInfo {
            id: hostname_or_default()?,
            endpoint,
            catalog_path: dir.join("default").join("catalog.sqlite").to_string(),
            mode: status.get("mode").cloned().context("instance status missing 'mode' — server not fully initialized")?,
            platform: sigiro::federation::PlatformInfo {
                os: status.get("platform_os").cloned().context("instance status missing 'platform_os' — server not fully initialized")?,
                kernel: status.get("platform_kernel").cloned().context("instance status missing 'platform_kernel' — server not fully initialized")?,
                arch: status.get("platform_arch").cloned().context("instance status missing 'platform_arch' — server not fully initialized")?,
                container: String::new(),
            },
            ebpf_status: status
                .get("ebpf_status")
                .cloned()
                .context("instance status missing 'ebpf_status' — server not fully initialized")?,
            capabilities: sigiro::federation::WorkerCapabilities {
                bpf: status
                    .get("capability_bpf")
                    .map(|v| v == "true")
                    .context("instance status missing 'capability_bpf' — server not fully initialized")?,
                net_admin: status
                    .get("capability_net_admin")
                    .map(|v| v == "true")
                    .context("instance status missing 'capability_net_admin' — server not fully initialized")?,
            },
            missing_permissions: vec![],
            suggestions: vec![],
            quack_port,
        };
        if let Err(e) = client.register_worker(&worker).await {
            tracing::warn!("failed to register with central node: {e:#}");
        }
    }

    tracing::info!("sigiro ready — data_dir={}", dir);
    // F-03: flip the typed init-complete flag so /health reports Ready.
    // The previous .ready file approach required a disk write on every
    // server start (silently dropped on failure) plus a disk read on every
    // health probe. In-memory state is faster, type-checked, and cannot
    // be forgotten by `run_serve` (we're already here at the tail).
    init_complete.store(true, std::sync::atomic::Ordering::Release);

    let shared_admin = Arc::new(Mutex::new(admin_conn));

    let router = Arc::new(MsgRouter::new(Arc::clone(&pool)));
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

fn hostname_or_default() -> Result<String> {
    sigiro::config::HOSTNAME
        .value()
        .map_err(anyhow::Error::from)
        .context("HOSTNAME env var not set — required for federation worker registration")
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

fn truncate(s: &str, len: usize) -> String {
    s.chars().take(len).collect()
}

#[derive(Debug)]
struct WaterfallSpan {
    span_id: String,
    parent_span_id: String,
    span_name: String,
    span_kind: i32,
    duration_us: i64,
    timestamp_us: i64,
    status_code: i32,
    service_name: String,
}

async fn run_waterfall(trace_id: &str) -> Result<()> {
    run_waterfall_direct(trace_id).await
}

fn format_us(us: i64) -> String {
    time::Duration::new(us / 1_000_000, (((us % 1_000_000) as u32) * 1000) as i32)
        .to_string()
        .replace(' ', "")
}

fn render_waterfall(spans: &[WaterfallSpan], trace_id: &str) {
    if spans.is_empty() {
        println!("No spans found for trace_id: {trace_id}");
        return;
    }

    let earliest_ts = spans.iter().map(|s| s.timestamp_us).min()
        .expect("at least one span is required to compute trace duration — spans list came from non-empty query");
    let latest_ts = spans
        .iter()
        .map(|s| s.timestamp_us.saturating_add(s.duration_us))
        .max()
        .expect("spans is non-empty (checked above), so max is valid");
    let total_duration_us = (latest_ts - earliest_ts).max(0);

    println!("Trace: {trace_id}");
    println!(
        "Spans: {}  Total duration: {}\n",
        spans.len(),
        format_us(total_duration_us)
    );

    let mut children: std::collections::BTreeMap<String, Vec<usize>> =
        std::collections::BTreeMap::new();
    for (i, span) in spans.iter().enumerate() {
        if !span.parent_span_id.is_empty() {
            children
                .entry(span.parent_span_id.clone())
                .or_default()
                .push(i);
        }
    }

    fn render_span(
        spans: &[WaterfallSpan],
        children: &std::collections::BTreeMap<String, Vec<usize>>,
        idx: usize,
        depth: usize,
        earliest_ts: i64,
    ) {
        let span = &spans[idx];
        let rel_start_us = span.timestamp_us as f64 - earliest_ts as f64;
        let duration_us = span.duration_us as f64;
        let end_us = rel_start_us + duration_us;

        let indent = "  ".repeat(depth);

        let kind_str = match span.span_kind {
            0 => "PRODUCER",
            1 => "INTERNAL",
            2 => "SERVER",
            3 => "CLIENT",
            4 => "CONSUMER",
            _ => "UNKNOWN",
        };

        let time_str = format_us(span.duration_us);

        let bar_max = 40usize;
        let bar = if duration_us > 0.0 {
            let start_bar = ((rel_start_us / end_us.max(1.0)) * bar_max as f64) as usize;
            let end_bar = bar_max;
            let filled = end_bar.saturating_sub(start_bar.max(0));
            format!(
                "{:>width$}",
                "█".repeat(filled.min(bar_max)),
                width = bar_max
            )
        } else {
            format!("{:>width$}", "", width = bar_max)
        };

        let error_tag = if span.status_code == 2 { " [ERR]" } else { "" };
        let service_name = format!("{}{}", truncate(&span.service_name, 10), error_tag);

        println!(
            "{indent}{} {:15} {:6} {:12} {} {time_str}",
            bar,
            truncate(&span.span_name, 15),
            kind_str,
            service_name,
            if span.parent_span_id.is_empty() {
                "ROOT"
            } else {
                ""
            },
        );

        if let Some(kids) = children.get(&span.span_id) {
            for &child_idx in kids {
                render_span(spans, children, child_idx, depth + 1, earliest_ts);
            }
        }
    }

    let root_spans: Vec<usize> = spans
        .iter()
        .enumerate()
        .filter(|(_, s)| s.parent_span_id.is_empty())
        .map(|(i, _)| i)
        .collect();

    for (i, root_idx) in root_spans.iter().enumerate() {
        if i > 0 {
            println!("  (concurrent root span {})", i + 1);
        }
        render_span(spans, &children, *root_idx, 0, earliest_ts);
    }
}

async fn run_waterfall_direct(trace_id: &str) -> Result<()> {
    let dir = data_dir()?;

    let project_dir = std::fs::read_dir(&dir)
        .context("failed to read data directory")?
        .filter_map(std::result::Result::ok)
        .filter_map(|e| camino::Utf8PathBuf::from_path_buf(e.path()).ok())
        .find(|p| p.is_dir() && p.join("catalog.sqlite").exists())
        .context("no DuckDB project found in data directory")?;

    let catalog_path = project_dir.join("catalog.sqlite");
    let project_name = project_dir
        .file_name()
        .context("project directory name is not valid UTF-8")?;

    if !catalog_path.exists() {
        anyhow::bail!(
            "No data found at {catalog_path}. Run sigiro serve and send it some traces first.",
        );
    }

    let conn = duckdb::Connection::open_in_memory().context("failed to open in-memory DuckDB")?;

    let ext_dir = dir.join("extensions");
    std::fs::create_dir_all(&ext_dir).context("failed to create extensions directory")?;
    let p = ext_dir.as_str();
    conn.execute_batch(&format!("SET extension_directory = '{p}';"))
        .context("failed to set extension_directory")?;
    conn.execute_batch("SET memory_limit = '256MB'; SET threads = 1;")
        .context("failed to set DuckDB limits")?;

    let catalog_str = catalog_path.as_str();
    sigiro::storage::install_extensions_with_progress(&conn, &["ducklake", "sqlite"])?;

    let attach_sql = format!("ATTACH 'ducklake:{catalog_str}' AS lake");
    if let Err(attach_err) = conn.execute_batch(&attach_sql) {
        if attach_err.to_string().contains("lock")
            || attach_err.to_string().contains("Conflicting lock")
        {
            anyhow::bail!(
                "DuckDB catalog is locked by a running sigiro server.\n\
                 Stop the server first, or use sigiro query --sql \"SELECT ... FROM lake.sigiro_spans WHERE trace_id = '{trace_id}'\"\n\
                 Alternatively, set SIGIRO_DATA_DIR to a different directory to query a separate instance."
            );
        }
        anyhow::bail!("failed to attach DuckLake catalog: {attach_err}");
    }

    let prefix = sigiro::storage::table_prefix();
    sigiro::schema::create_tables(&conn, prefix).context("failed to create/check schema")?;

    let sql = format!(
        "SELECT span_id, parent_span_id, span_name, span_kind, duration,
                {prefix}sigiro_spans.timestamp, status_code, service_name
         FROM {prefix}sigiro_spans
         WHERE trace_id = ?
         ORDER BY {prefix}sigiro_spans.timestamp",
    );

    let mut stmt = conn.prepare(&sql).context("failed to prepare query")?;
    let mut rows = stmt
        .query(duckdb::params![trace_id])
        .context("failed to query spans")?;

    let mut spans: Vec<WaterfallSpan> = Vec::new();
    while let Some(row) = rows.next().context("failed to read span row")? {
        spans.push(WaterfallSpan {
            span_id: row.get(0).context("span_id")?,
            parent_span_id: row.get(1).unwrap_or_default(),
            span_name: row.get(2).context("span_name")?,
            span_kind: row.get(3).context("span_kind")?,
            duration_us: row.get(4).context("duration")?,
            timestamp_us: row.get(5).context("timestamp")?,
            status_code: row.get(6).context("status_code")?,
            service_name: row.get(7).context("service_name")?,
        });
    }

    println!("Project: {project_name}  Trace: {trace_id}");
    render_waterfall(&spans, trace_id);
    Ok(())
}

// ---------------------------------------------------------------------------
// Topology command — ASCII dependency graph
// ---------------------------------------------------------------------------

async fn run_topology(service: Option<&str>, format: TopologyFormat) -> Result<()> {
    let client = require_client()?;

    // Get services list
    let services_resp = client.services().await?;
    let all_services: Vec<&str> = services_resp
        .services
        .iter()
        .map(|s| s.name.as_str())
        .collect();

    // Get dependency data from investigate
    let services_to_query: Vec<String> = match service {
        Some(s) => vec![s.to_string()],
        None => all_services
            .iter()
            .take(20)
            .map(std::string::ToString::to_string)
            .collect(),
    };

    let mut dep_map: HashMap<String, HashSet<String>> = HashMap::new();
    let mut rev_dep_map: HashMap<String, HashSet<String>> = HashMap::new();

    // Query each service for its dependencies
    for svc_name in &services_to_query {
        if let Ok(mut blocks) = client
            .investigate_multi(Some(svc_name), None, 0, i64::MAX)
            .await
        {
            if let Some(block) = blocks.remove(svc_name) {
                // Extract dependency service names
                let deps: HashSet<String> = block
                    .context
                    .dependencies
                    .into_iter()
                    .map(|dep| dep.service_name)
                    .collect();
                let dependents: HashSet<String> = block
                    .context
                    .dependents
                    .into_iter()
                    .map(|dep| dep.service_name)
                    .collect();

                dep_map.insert(svc_name.clone(), deps);
                rev_dep_map.insert(svc_name.clone(), dependents);
            }
        }
    }

    match format {
        TopologyFormat::Ascii => {
            render_topology_ascii(&dep_map, &rev_dep_map, service, &all_services);
        }
        TopologyFormat::Mermaid => {
            render_topology_mermaid(&dep_map, &rev_dep_map, service, &all_services);
        }
    }

    Ok(())
}

fn render_topology_ascii(
    deps: &HashMap<String, HashSet<String>>,
    _rev_deps: &HashMap<String, HashSet<String>>,
    focus: Option<&str>,
    all_services: &[&str],
) {
    println!("Service Dependencies");
    println!("====================");
    println!();

    // Find root services (no incoming dependencies)
    let all_deps: HashSet<&str> = deps
        .values()
        .flat_map(|s| s.iter().map(std::string::String::as_str))
        .collect();

    let roots: Vec<&&str> = all_services
        .iter()
        .filter(|s| !all_deps.contains(*s))
        .collect();

    // Build children map
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    for (svc, dep_set) in deps {
        for dep in dep_set {
            children.entry(dep.clone()).or_default().push(svc.clone());
        }
    }

    // Render from roots or focus service
    if let Some(focus_svc) = focus {
        render_service_tree(focus_svc, deps, &children, 0, &mut HashSet::new());
    } else {
        for root in &roots {
            render_service_tree(root, deps, &children, 0, &mut HashSet::new());
        }
    }
}

fn render_service_tree(
    svc: &str,
    deps: &HashMap<String, HashSet<String>>,
    children: &HashMap<String, Vec<String>>,
    depth: usize,
    visited: &mut HashSet<String>,
) {
    if visited.contains(svc) {
        println!("{}└─ {svc} (cycle)", "  ".repeat(depth));
        return;
    }
    visited.insert(svc.to_string());

    let dependencies: Vec<&String> = deps
        .get(svc)
        .map(|s| s.iter().collect())
        .unwrap_or_default();
    let deps_str = if dependencies.is_empty() {
        String::new()
    } else {
        format!(
            " → [{}]",
            dependencies.iter().map(|s| s.as_str()).join(", ")
        )
    };
    println!("{}└─ {svc}{deps_str}", "  ".repeat(depth));

    if let Some(kids) = children.get(svc) {
        for child in kids {
            render_service_tree(child, deps, children, depth + 1, visited);
        }
    }
}

fn render_topology_mermaid(
    deps: &HashMap<String, HashSet<String>>,
    _rev_deps: &HashMap<String, HashSet<String>>,
    focus: Option<&str>,
    all_services: &[&str],
) {
    println!("```mermaid");
    println!("flowchart LR");
    println!("    direction LR");

    // Collect all nodes
    let mut nodes: HashSet<&str> = HashSet::new();
    for svc in all_services {
        nodes.insert(svc);
    }
    for (svc, dep_set) in deps {
        nodes.insert(svc.as_str());
        for dep in dep_set {
            nodes.insert(dep.as_str());
        }
    }

    // Add subgraph for focus or all services
    let focus_str = focus.unwrap_or("services");
    println!("    subgraph {focus_str}");

    let sorted_nodes: Vec<_> = nodes.into_iter().sorted().collect();

    for node in &sorted_nodes {
        println!("        {node}[{node}]");
    }
    println!("    end");

    // Add edges
    let mut edges: HashSet<(String, String)> = HashSet::new();
    for (svc, dep_set) in deps {
        for dep in dep_set {
            edges.insert((svc.clone(), dep.clone()));
        }
    }

    for (from, to) in edges {
        println!("    {from} --> {to}");
    }

    println!("```");
}
// ---------------------------------------------------------------------------
// TraceTree command — hierarchical trace tree
// ---------------------------------------------------------------------------

async fn run_trace_tree(trace_id: &str) -> Result<()> {
    let client = require_client()?;

    let trace = client.trace(trace_id).await?;

    if trace.spans.is_empty() {
        println!("No spans found for trace: {trace_id}");
        return Ok(());
    }

    println!("Trace: {trace_id}");
    println!(
        "Spans: {}  Duration: {:.2}ms\n",
        trace.span_count,
        trace.total_duration_us as f64 / 1_000_000.0
    );

    // Build span map
    let mut span_map: HashMap<String, SpanDetail> = HashMap::new();
    for span in &trace.spans {
        span_map.insert(span.span_id.clone(), span.clone());
    }

    // Find root spans (no parent or parent not in trace)
    let root_spans: Vec<_> = trace
        .spans
        .iter()
        .filter(|s| {
            s.parent_span_id.is_none()
                || s.parent_span_id
                    .as_ref()
                    .map(|p| !span_map.contains_key(p))
                    .unwrap_or(true)
        })
        .collect();

    // Render tree for each root span
    for (i, root) in root_spans.iter().enumerate() {
        if i > 0 {
            println!();
        }
        render_trace_tree_recursive(&span_map, root, 0, &mut HashSet::new());
    }

    Ok(())
}

fn render_trace_tree_recursive(
    spans: &HashMap<String, SpanDetail>,
    span: &SpanDetail,
    depth: usize,
    rendered: &mut HashSet<String>,
) {
    if rendered.contains(&span.span_id) {
        return;
    }
    rendered.insert(span.span_id.clone());

    let indent = if depth == 0 {
        String::new()
    } else {
        let prefix = if depth == 1 { "└─ " } else { "  " };
        format!("{}{prefix}", "   ".repeat(depth - 1))
    };

    let pipe = if depth > 0 {
        if depth == 1 { "├─ " } else { "│  " }
    } else {
        ""
    };

    let duration_ms = span.duration_us as f64 / 1_000_000.0;
    let error_tag = if span.status_code == 2 { " [ERR]" } else { "" };

    // Find children
    let children: Vec<_> = spans
        .values()
        .filter(|s| s.parent_span_id.as_ref() == Some(&span.span_id))
        .collect();

    let connector = if children.is_empty() {
        "└─ "
    } else {
        "├─ "
    };

    println!(
        "{indent}{pipe}{connector}{} [{}] {:.2}ms{}",
        span.span_name, span.service_name, duration_ms, error_tag
    );

    // Render children
    for child in &children {
        render_trace_tree_recursive(spans, child, depth + 1, rendered);
    }
}
