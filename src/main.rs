use anyhow::{Context, Result};
use clap::{Parser, Subcommand, builder::styling};
use std::sync::{Arc, Mutex};

use sigiro::toon::to_toon;
use sigiro::{data_dir, inserter::MsgRouter, pool::ProjectPool, profiling::start_profiling};

const STYLES: styling::Styles = styling::Styles::styled()
    .header(styling::AnsiColor::Cyan.on_default().bold())
    .usage(styling::AnsiColor::Cyan.on_default().bold())
    .literal(styling::AnsiColor::Green.on_default().bold())
    .placeholder(styling::AnsiColor::Yellow.on_default());

const FEDERATION_LOCAL_IP: &str = "127.0.0.1";

#[derive(Parser)]
#[command(name = "sigiro", version, about = "Observability for AI agents", styles = STYLES)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Output format (toon or json)
    #[arg(long, default_value = "toon")]
    output: OutputFormat,
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
        /// Disable eBPF auto-instrumentation (run in OTLP-only mode)
        #[arg(long)]
        no_ebpf: bool,
        /// Run as central federation node (spawns Spice.ai for distributed query)
        #[arg(long)]
        central: bool,
        /// URL of central node for collector self-registration (e.g. http://central:9999)
        #[arg(long)]
        central_url: Option<String>,
    },
    /// Run SQL query
    Query {
        /// SQL statement to execute
        sql: String,
    },
    /// Show server health
    Status,
    /// Investigate a service — returns structured metrics, spans, logs, linked traces
    Investigate {
        /// Service name to investigate
        service: String,
        /// Start of range in epoch seconds (defaults to now - 900)
        #[arg(long)]
        from: Option<i64>,
        /// End of range in epoch seconds (defaults to now)
        #[arg(long)]
        to: Option<i64>,
    },
    /// Show trace waterfall — parent/child span tree with timing
    Waterfall {
        /// Trace ID (hex string) to display
        trace_id: String,
    },
    /// Start MCP stdio server for Claude/Cursor/Codex integration
    Mcp,
    /// Save API key for hosted mode
    Auth {
        /// API key (e.g. sigiro_...)
        key: String,
    },
    /// Admin commands for key management
    Admin {
        #[command(subcommand)]
        action: AdminAction,
    },
}

#[derive(Subcommand)]
enum AdminAction {
    /// Create a new API key for a project
    CreateKey {
        /// Project ID
        #[arg(long)]
        project: String,
        /// Plan (free, builder, team, pro)
        #[arg(long, default_value = "free")]
        plan: String,
        /// Rate limit (requests per minute)
        #[arg(long, default_value = "100")]
        rate_limit: u32,
    },
    /// Revoke an API key by short token
    RevokeKey {
        /// Short token portion of the key
        short_token: String,
    },
}

/// Format output
fn format_output(value: &serde_json::Value, output_format: &OutputFormat) -> Result<String> {
    match output_format {
        OutputFormat::Toon => Ok(to_toon(value)),
        OutputFormat::Json => Ok(serde_json::to_string_pretty(value)?),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // OTel self-instrumentation: if OTEL_EXPORTER_OTLP_ENDPOINT is set,
    // the subscriber is initialized with an OTel layer in init_telemetry().
    // Otherwise, falls back to the standard fmt subscriber.
    let _telemetry_guard = match sigiro::otel::init_telemetry() {
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
            no_ebpf,
            central,
            central_url,
        }) => {
            run_serve(no_ebpf, central, central_url).await?;
        }
        None => {
            run_serve(false, false, None).await?;
        }
        Some(Command::Query { sql }) => {
            let client = require_client()?;
            let result = client.query(&sql).await?;
            println!(
                "{}",
                format_output(&serde_json::to_value(&result)?, &cli.output)?
            );
        }
        Some(Command::Status) => {
            let client = require_client()?;
            let result = client.status().await?;
            println!("{}", format_output(&serde_json::to_value(&result)?, &cli.output)?);
        }
        Some(Command::Investigate { service, from, to }) => {
            let client = require_client()?;
            let block = client
                .investigate(
                    &service,
                    from.map(|s| s * 1_000_000),
                    to.map(|s| s * 1_000_000),
                )
                .await?;
            println!(
                "{}",
                format_output(&serde_json::to_value(&block)?, &cli.output)?
            );
        }
        Some(Command::Waterfall { trace_id }) => {
            run_waterfall(&trace_id).await?;
        }
        Some(Command::Mcp) => {
            let client = require_client()?;
            tracing::info!("MCP server connecting to {}", client.base_url());
            sigiro::mcp::run_mcp_server(client).await?;
        }
        Some(Command::Auth { key }) => {
            let path = sigiro::config::save_api_key(&key)?;
            println!("API key saved to {}", path.display());
        }
        Some(Command::Admin { action }) => {
            // Try HTTP to running server first (solves DuckDB single-writer lock).
            // Fall back to direct DuckDB if server isn't running (bootstrapping).
            let server_url = sigiro::config::ENDPOINT.value().map_err(|e| anyhow::anyhow!("{e}"))?;

            match action {
                AdminAction::CreateKey {
                    project,
                    plan,
                    rate_limit,
                } => {
                    let client = sigiro::client::SigiroClient::new(&server_url, "");
                    match client.admin_create_key(&project, &plan, rate_limit).await {
                        Ok(raw_key) => {
                            println!("{raw_key}");
                        }
                        Err(http_err) => {
                            anyhow::bail!(
                                "server not reachable at {server_url}: {http_err:#} — start the sigiro server first, or ensure SIGIRO_ENDPOINT points to a running instance"
                            );
                        }
                    }
                }
                AdminAction::RevokeKey { short_token } => {
                    let client = sigiro::client::SigiroClient::new(&server_url, "");
                    match client.admin_revoke_key(&short_token).await {
                        Ok(true) => {
                            println!("Key revoked: {short_token}");
                        }
                        Ok(false) => {
                            println!("No key found with short token: {short_token}");
                        }
                        Err(http_err) => {
                            anyhow::bail!(
                                "server not reachable at {server_url}: {http_err:#} — start the sigiro server first, or ensure SIGIRO_ENDPOINT points to a running instance"
                            );
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

async fn run_serve(no_ebpf: bool, central: bool, central_url: Option<String>) -> Result<()> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir)?;

    // MsgRouter: handlers send directly to per-project flusher tasks.
    let pool = Arc::new(ProjectPool::new(dir.clone(), 100));
    let admin_conn = sigiro::admin::open_admin_db()?;

    // Write instance status at startup
    sigiro::admin::write_instance_status(&admin_conn, "version", env!("CARGO_PKG_VERSION"))?;
    sigiro::admin::write_instance_status(&admin_conn, "mode", "otlp")?;
    sigiro::admin::write_instance_status(&admin_conn, "platform_os", std::env::consts::OS)?;
    sigiro::admin::write_instance_status(&admin_conn, "platform_arch", std::env::consts::ARCH)?;

    // Extension probe is deferred to first project connection (pool.create_project).
    // Use `sigiro fetch-extensions` to pre-download for offline deployments.

    // Pre-create dev_project so queries work without ingest
    match pool.create_project("dev_project") {
        Ok(_) => tracing::info!("dev_project initialized"),
        Err(e) => tracing::warn!("dev_project init failed (non-fatal): {e}"),
    }

    // eBPF auto-instrumentation: try obi, degrade gracefully
    if no_ebpf {
        tracing::info!("eBPF disabled by --no-ebpf flag");
        sigiro::admin::write_instance_status(&admin_conn, "ebpf_status", "disabled")?;
        sigiro::admin::write_instance_status(&admin_conn, "ebpf_reason", "--no-ebpf flag")?;
    } else {
        let otlp_endpoint = sigiro::config::OTEL_ENDPOINT.value().map_err(|e| anyhow::anyhow!("{e}"))?;
        let status = sigiro::ebpf::ebpf_status("obi");
        sigiro::ebpf::write_ebpf_status(&admin_conn, &status)?;
        match status.status.as_str() {
            "ready" => {
                sigiro::ebpf::start_obi("obi", &otlp_endpoint)?;
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

    // Federation: central node mode
    if central {
        tracing::info!("running as central federation node");
        sigiro::federation::start_spice(&dir)?;
    }

    // Federation: collector self-registration with central node
    if let Some(ref url) = central_url {
        tracing::info!("collector self-registering with central node at {url}");
        let client = sigiro::client::SigiroClient::new(url, "");
        let status = sigiro::admin::read_all_status(&admin_conn)?;
        let worker = sigiro::federation::WorkerInfo {
            id: hostname_or_default()?,
            endpoint: format!("http://{FEDERATION_LOCAL_IP}:9999"),
            catalog_path: dir.join("default").join("catalog.sqlite")
                .to_str().context("catalog_path contains non-UTF8 characters")?.to_string(),
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
        };
        if let Err(e) = client.register_worker(&worker).await {
            tracing::warn!("failed to register with central node: {e:#}");
        }
    }

    tracing::info!("sigiro ready — data_dir={}", dir.display());

    let shared_admin = Arc::new(Mutex::new(admin_conn));

    let router = Arc::new(MsgRouter::new(Arc::clone(&pool)));

    // Evict idle connections every 2 minutes
    const EVICT_INTERVAL_SECS: u64 = 120;
    const MAX_IDLE_SECS: u64 = 300;
    let evictor_pool = Arc::clone(&pool);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(EVICT_INTERVAL_SECS));
        loop {
            interval.tick().await;
            evictor_pool.evict_idle(std::time::Duration::from_secs(MAX_IDLE_SECS));
        }
    });

    sigiro::server::run_servers(pool, shared_admin, router).await?;
    Ok(())
}

/// Build HTTP client from config. Fails fast if API key or endpoint is not configured.
fn require_client() -> Result<sigiro::client::SigiroClient> {
    sigiro::client::try_build_client()
}

fn hostname_or_default() -> Result<String> {
    sigiro::config::HOSTNAME.value().map_err(|e| anyhow::anyhow!("{e}"))
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

    let earliest_ts = spans.iter().map(|s| s.timestamp_us).min().unwrap_or(0);
    let latest_ts = spans
        .iter()
        .map(|s| {
            s.timestamp_us + s.duration_us
        })
        .max()
        .unwrap_or(earliest_ts);
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
            format!("{:>width$}", "█".repeat(filled.min(bar_max)), width = bar_max)
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
            if span.parent_span_id.is_empty() { "ROOT" } else { "" },
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
        .map(|e| e.path())
        .find(|p| p.is_dir() && p.join("catalog.sqlite").exists())
        .context("no DuckDB project found in data directory")?;

    let catalog_path = project_dir.join("catalog.sqlite");
    let project_name = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    if !catalog_path.exists() {
        anyhow::bail!(
            "No data found at {}. Run sigiro serve and send it some traces first.",
            catalog_path.display()
        );
    }

    let conn = duckdb::Connection::open_in_memory()
        .context("failed to open in-memory DuckDB")?;

    let ext_dir = dir.join("extensions");
    std::fs::create_dir_all(&ext_dir).context("failed to create extensions directory")?;
    if let Some(p) = ext_dir.to_str() {
        conn.execute_batch(&format!("SET extension_directory = '{p}';"))
            .context("failed to set extension_directory")?;
    }
    conn.execute_batch("SET memory_limit = '256MB'; SET threads = 1;")
        .context("failed to set DuckDB limits")?;

    let catalog_str = catalog_path.to_string_lossy();
    conn.execute_batch("INSTALL ducklake; INSTALL sqlite; LOAD ducklake; LOAD sqlite;")
        .context("failed to load DuckLake extensions")?;

    let attach_sql = format!("ATTACH 'ducklake:{catalog_str}' AS lake");
    if let Err(attach_err) = conn.execute_batch(&attach_sql) {
        if attach_err.to_string().contains("lock") || attach_err.to_string().contains("Conflicting lock") {
            anyhow::bail!(
                "DuckDB catalog is locked by a running sigiro server.\n\
                 Stop the server first, or use sigiro query --sql \"SELECT ... FROM lake.sigiro_spans WHERE trace_id = '{trace_id}'\"\n\
                 Alternatively, set SIGIRO_DATA_DIR to a different directory to query a separate instance."
            );
        }
        anyhow::bail!("failed to attach DuckLake catalog: {attach_err}");
    }

    let prefix = sigiro::storage::table_prefix();
    sigiro::schema::create_tables(&conn, prefix)
        .context("failed to create/check schema")?;

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
